//! Audio decoder front end.
//!
//! Mirror of [`encode::Encoder`](crate::encode::Encoder): dispatches over the
//! catalog codec and produces interleaved `f32` PCM.

use std::time::Duration;

use unsafe_libopus::{
	OPUS_OK, OPUS_RESET_STATE, OpusDecoder, opus_decode_float, opus_decoder_create, opus_decoder_ctl_impl,
	opus_decoder_destroy, varargs,
};

#[cfg(feature = "aac")]
use symphonia_core::codecs::audio::AudioDecoder;

use super::Decoded;
#[cfg(feature = "aac")]
use crate::aac;
use crate::opus;
use crate::pcm;
use crate::{Activity, Error, Format};

/// Opus packets cap at 120 ms (RFC 6716 §2.1.4).
const MAX_FRAME_MS: usize = 120;

/// Where a decoder starts on a track that already holds groups.
///
/// A track keeps its groups for a while after they are read, so a decoder does
/// not always open on an empty one: a player rebuilding its decoder subscribes
/// while its predecessor still holds groups, and a rendition switched away from
/// and back to stays warm for the track's idle linger. What to do with that
/// backlog depends on the consumer, and the two answers are opposites, so it is
/// asked rather than guessed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Start {
	/// The oldest group the track still holds, decoding everything cached.
	///
	/// What a recorder, an export, or anything reading a complete track wants,
	/// and the default because dropping media a caller has not asked to drop is
	/// the worse mistake.
	#[default]
	Oldest,
	/// The newest group, skipping whatever is already cached.
	///
	/// What a live player wants. Without it a rebuilt decoder walks the whole
	/// backlog at decode speed before reaching live media, which a viewer sees
	/// as playback jumping backwards and then sprinting to catch up.
	Latest,
}

/// Decoder configuration: the PCM layout to emit, plus the subscription's
/// latency budget.
///
/// The mirror of [`encode::Config`](crate::encode::Config): it describes the
/// output, since the codec's own shape is read from the catalog.
///
/// `#[non_exhaustive]`: build via [`Config::new`] (or `default()`) and set the
/// optional fields, so future knobs don't break callers.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Config {
	/// How to pack samples in each emitted frame.
	pub format: Format,
	/// Sample rate to emit at. `None` uses the codec's native rate from the
	/// catalog; anything else resamples.
	pub sample_rate: Option<u32>,
	/// Channel count to emit. `None` uses the codec's native count; anything
	/// else remixes mono and stereo at the decode boundary.
	pub channels: Option<u32>,
	/// Upper bound on buffering before skipping a stalled group.
	///
	/// Forwarded to [`moq_mux::container::Consumer::with_latency`]: if a group is
	/// stuck and a newer group is more than this far ahead, the consumer skips.
	/// `None` keeps the moq-mux default of zero, which skips aggressively. Set it
	/// to the playout buffer you can tolerate (typically tens to a few hundred ms)
	/// for the best congestion-vs-quality trade-off. The `_max` suffix is a
	/// reminder that we never *add* latency here: the consumer skips only when
	/// newer data is already this far ahead. A companion `latency_min` for
	/// jitter-buffer padding will land in a follow-up.
	pub latency_max: Option<Duration>,
	/// Where to start on a track that already holds groups.
	pub start: Start,
	/// How much of a hole to fill with recovered audio before leaving it a hole.
	///
	/// Zero (the default) leaves every hole in the output, as
	/// [`read`](Consumer::read) documents. Up to this much of an undeclared hole
	/// is recovered instead: the lost frames conceal, and the one before the
	/// arriving packet comes back from its in-band forward error copy when the
	/// sender coded one. Longer holes stay holes, since hiding a skipped group
	/// is worse than showing it.
	pub conceal: Duration,
}

impl Config {
	/// A default config: the codec's native rate and channel count, interleaved
	/// `f32`, and the moq-mux default latency.
	pub fn new() -> Self {
		Self::default()
	}
}

/// Decodes codec packets into interleaved `f32` PCM.
///
/// The bring-your-own-payload layer under [`Consumer`](super::Consumer): use it
/// when the packets don't come from a plain track subscription.
pub struct Decoder {
	backend: Backend,
	sample_rate: u32,
	channel_count: u32,
	delay: usize,
}

enum Backend {
	Opus(Opus),
	Pcm {
		bytes_per_frame: usize,
	},
	#[cfg(feature = "aac")]
	Aac(Box<Aac>),
}

struct Opus {
	inner: *mut OpusDecoder,
	pre_skip_remaining: usize,
	max_frame_size: usize,
	/// Frames per channel in the last packet that decoded, which is what a lost
	/// packet most likely carried. `None` until one has.
	last_frame_size: Option<usize>,
	in_dtx: bool,
}

// SAFETY: see Encoder.
unsafe impl Send for Opus {}

/// Boxed in [`Backend`]: the symphonia decoder carries its own filterbank state,
/// which is far larger than the other backends' handles.
#[cfg(feature = "aac")]
struct Aac {
	inner: symphonia_codec_aac::AacDecoder,
}

impl Decoder {
	/// Build a decoder from a catalog [`AudioConfig`](hang::catalog::AudioConfig).
	///
	/// Parses the OpusHead `description` if present; falls back to the catalog's
	/// declared sample rate / channel count. PCM uses those catalog fields
	/// directly and requires an absent `description`.
	pub fn new(catalog: &hang::catalog::AudioConfig) -> Result<Self, Error> {
		match &catalog.codec {
			hang::catalog::AudioCodec::Opus => Self::new_opus(catalog),
			hang::catalog::AudioCodec::Pcm => Self::new_pcm(catalog),
			#[cfg(feature = "aac")]
			hang::catalog::AudioCodec::AAC(aac) => Self::new_aac(catalog, aac.profile),
			codec => Err(Error::Unsupported(format!("unsupported audio codec: {codec}"))),
		}
	}

	fn new_opus(catalog: &hang::catalog::AudioConfig) -> Result<Self, Error> {
		let (sample_rate, channel_count, pre_skip) = if let Some(desc) = &catalog.description {
			let mut buf = desc.as_ref();
			match moq_mux::codec::opus::Config::parse(&mut buf) {
				Ok(head) => (head.sample_rate, head.channel_count, head.pre_skip),
				Err(_) => (catalog.sample_rate, catalog.channel_count, 0),
			}
		} else {
			(catalog.sample_rate, catalog.channel_count, 0)
		};

		opus::validate_rate(sample_rate)?;
		let channels = opus::validate_channels(channel_count)?;

		let mut err = 0i32;
		// SAFETY: out-pointer is valid; inner is checked for null below.
		let inner = unsafe { opus_decoder_create(sample_rate as i32, channels, &mut err) };
		if err != OPUS_OK || inner.is_null() {
			return Err(opus::error(err, "opus_decoder_create"));
		}

		let max_frame_size = (sample_rate as usize * MAX_FRAME_MS) / 1000;
		let pre_skip_remaining = (pre_skip as usize * sample_rate as usize) / 48_000;

		Ok(Self {
			backend: Backend::Opus(Opus {
				inner,
				pre_skip_remaining,
				max_frame_size,
				last_frame_size: None,
				in_dtx: false,
			}),
			sample_rate,
			channel_count,
			delay: pre_skip_remaining,
		})
	}

	/// AAC-LC only, which is what every gateway that feeds this crate publishes.
	///
	/// HE-AAC is rejected however its config spells it: leading with SBR or PS
	/// (mp4a.40.5 / .29), or leading with LC and declaring SBR in a sync extension
	/// after the core. Symphonia decodes no SBR either way, so the alternative is
	/// half-rate audio that sounds like a fault rather than an unsupported codec.
	/// A stream that signals SBR only in band is indistinguishable from LC in the
	/// config, and does decode as the core.
	#[cfg(feature = "aac")]
	fn new_aac(catalog: &hang::catalog::AudioConfig, profile: u8) -> Result<Self, Error> {
		use symphonia_core::codecs::audio::well_known::CODEC_ID_AAC;
		use symphonia_core::codecs::audio::{AudioCodecParameters, AudioDecoderOptions};

		let description = aac::description(catalog, profile)?;

		let mut params = AudioCodecParameters::new();
		params
			.for_codec(CODEC_ID_AAC)
			.with_extra_data(description.to_vec().into_boxed_slice());

		let inner = symphonia_codec_aac::AacDecoder::try_new(&params, &AudioDecoderOptions::default())
			.map_err(|err| Error::Unsupported(format!("aac decoder: {err}")))?;

		// Resolved by the decoder from the config, so this is what it will emit
		// even when the catalog's own fields say otherwise.
		let params = inner.codec_params();
		let sample_rate = params
			.sample_rate
			.ok_or_else(|| Error::Unsupported("aac config declares no sample rate".into()))?;
		let channel_count = params
			.channels
			.as_ref()
			.map(|channels| channels.count())
			.ok_or_else(|| Error::Unsupported("aac config declares no channels".into()))?;

		Ok(Self {
			backend: Backend::Aac(Box::new(Aac { inner })),
			sample_rate,
			channel_count: channel_count as u32,
			delay: 0,
		})
	}

	fn new_pcm(catalog: &hang::catalog::AudioConfig) -> Result<Self, Error> {
		if catalog.sample_rate == 0 {
			return Err(Error::Unsupported("pcm sample rate must be greater than zero".into()));
		}
		if catalog.channel_count == 0 {
			return Err(Error::Unsupported("pcm channel count must be greater than zero".into()));
		}
		if catalog.description.is_some() {
			return Err(Error::Unsupported("pcm catalog description must be absent".into()));
		}
		let bitrate = pcm::bitrate(catalog.sample_rate, catalog.channel_count)?;
		if catalog.bitrate.is_some_and(|declared| declared != bitrate) {
			return Err(Error::Unsupported(format!(
				"pcm catalog bitrate must be {bitrate} bits per second"
			)));
		}
		let bytes_per_frame = pcm::frame_bytes(1, catalog.channel_count)?;

		Ok(Self {
			backend: Backend::Pcm { bytes_per_frame },
			sample_rate: catalog.sample_rate,
			channel_count: catalog.channel_count,
			delay: 0,
		})
	}

	/// The rate the codec decodes at, read from the catalog.
	pub fn sample_rate(&self) -> u32 {
		self.sample_rate
	}

	/// The channel count the codec decodes at, read from the catalog.
	pub fn channel_count(&self) -> u32 {
		self.channel_count
	}

	/// Reset codec history and reapply startup delay for a new discontinuous epoch.
	pub fn reset(&mut self) -> Result<(), Error> {
		self.reset_prediction()?;
		if let Backend::Opus(opus) = &mut self.backend {
			opus.pre_skip_remaining = self.delay;
		}
		Ok(())
	}

	/// Reset codec prediction after packet loss without reapplying stream startup delay.
	pub(super) fn reset_prediction(&mut self) -> Result<(), Error> {
		match &mut self.backend {
			Backend::Opus(opus) => {
				// SAFETY: `inner` owns a live decoder and OPUS_RESET_STATE takes no arguments.
				let rc = unsafe { opus_decoder_ctl_impl(opus.inner, OPUS_RESET_STATE, varargs![]) };
				if rc != OPUS_OK {
					return Err(crate::opus::error(rc, "OPUS_RESET_STATE"));
				}
				opus.in_dtx = false;
			}
			Backend::Pcm { .. } => {}
			#[cfg(feature = "aac")]
			Backend::Aac(aac) => aac.inner.reset(),
		}
		Ok(())
	}

	/// How much startup delay is still to be trimmed, in native-rate frames.
	///
	/// Trimmed samples are media the packet covered even though nothing came out of
	/// it, so a caller tracking where a packet ends has to add back whatever this
	/// dropped across the call.
	pub(super) fn delay_remaining(&self) -> usize {
		match &self.backend {
			Backend::Opus(opus) => opus.pre_skip_remaining,
			Backend::Pcm { .. } => 0,
			#[cfg(feature = "aac")]
			Backend::Aac(_) => 0,
		}
	}

	/// Decode one packet into interleaved `f32` PCM and report its codec activity.
	///
	/// Empty Opus packets invoke packet-loss concealment. Loss during DTX remains
	/// classified as DTX, while loss during active audio remains active.
	pub fn decode(&mut self, packet: &[u8]) -> Result<Decoded, Error> {
		match &mut self.backend {
			Backend::Opus(opus) => {
				let mut out = vec![0.0f32; opus.max_frame_size * self.channel_count as usize];
				// SAFETY: `inner` owns a live OpusDecoder; packet/out slices are
				// bounded by the lengths we pass.
				let samples = unsafe {
					opus_decode_float(
						&mut *opus.inner,
						packet.as_ptr(),
						packet.len() as i32,
						out.as_mut_ptr(),
						opus.max_frame_size as i32,
						0,
					)
				};
				if samples < 0 {
					return Err(crate::opus::decode_error(samples));
				}
				if !packet.is_empty() {
					opus.last_frame_size = Some(samples as usize);
				}
				out.truncate(samples as usize * self.channel_count as usize);
				let trim_frames = opus.pre_skip_remaining.min(samples as usize);
				if trim_frames > 0 {
					let trim_samples = trim_frames * self.channel_count as usize;
					out.copy_within(trim_samples.., 0);
					out.truncate(out.len() - trim_samples);
					opus.pre_skip_remaining -= trim_frames;
				}
				let activity = crate::opus::activity(packet, opus.in_dtx);
				opus.in_dtx = activity.is_dtx();
				Ok(Decoded { samples: out, activity })
			}
			Backend::Pcm { bytes_per_frame } => {
				if packet.is_empty() || !packet.len().is_multiple_of(*bytes_per_frame) {
					return Err(Error::Misaligned {
						got: packet.len(),
						expected: packet.len().max(1).next_multiple_of(*bytes_per_frame),
					});
				}

				let out = packet
					.chunks_exact(pcm::BYTES_PER_SAMPLE)
					.map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]))
					.collect();
				Ok(Decoded {
					samples: out,
					activity: Activity::Active,
				})
			}
			#[cfg(feature = "aac")]
			Backend::Aac(aac) => {
				// The packet is a raw AAC frame, not ADTS, so there is nothing to
				// timestamp it with here: the container carries the timestamp and the
				// decoder only reads the payload.
				let packet = symphonia_core::packet::PacketRef::new(
					0,
					symphonia_core::units::Timestamp::ZERO,
					symphonia_core::units::Duration::ZERO,
					packet,
				);

				let decoded = aac
					.inner
					.decode_ref(&packet)
					.map_err(|err| Error::Decode(format!("aac: {err}")))?;

				let mut out = Vec::new();
				decoded.copy_to_vec_interleaved(&mut out);
				Ok(Decoded {
					samples: out,
					activity: Activity::Active,
				})
			}
		}
	}

	/// Recover one lost packet: the in-band forward error copy of the previous
	/// frame `next` carries when it has one, concealment when it does not (and
	/// when `next` is `None`). libopus conceals when the `decode_fec` request
	/// finds no copy, so one call covers both.
	///
	/// The recovered length is the last packet's, the same assumption the
	/// concealment path makes, and the copy decodes at the reduced rate the
	/// sender coded it at. Classified as the loss it covers rather than as
	/// `next`, whose own activity says nothing about the frame before it.
	pub fn recover(&mut self, next: Option<&[u8]>) -> Result<Decoded, Error> {
		let opus = match &mut self.backend {
			Backend::Opus(opus) => opus,
			_ => return Err(Error::Unsupported("only opus recovers lost packets".into())),
		};
		let frame_size = opus.last_frame_size.ok_or_else(|| {
			Error::Decode("opus packet lost before any packet decoded, so no length to conceal".into())
		})?;
		let (packet, fec) = match next {
			Some(next) if !next.is_empty() => (next, 1),
			_ => (&[][..], 0),
		};
		let mut out = vec![0.0f32; frame_size * self.channel_count as usize];
		// SAFETY: `inner` owns a live OpusDecoder; packet/out slices are
		// bounded by the lengths we pass.
		let samples = unsafe {
			opus_decode_float(
				&mut *opus.inner,
				packet.as_ptr(),
				packet.len() as i32,
				out.as_mut_ptr(),
				frame_size as i32,
				fec,
			)
		};
		if samples < 0 {
			return Err(crate::opus::decode_error(samples));
		}
		out.truncate(samples as usize * self.channel_count as usize);
		let trim_frames = opus.pre_skip_remaining.min(samples as usize);
		if trim_frames > 0 {
			let trim_samples = trim_frames * self.channel_count as usize;
			out.copy_within(trim_samples.., 0);
			out.truncate(out.len() - trim_samples);
			opus.pre_skip_remaining -= trim_frames;
		}
		let activity = crate::opus::activity(&[], opus.in_dtx);
		opus.in_dtx = activity.is_dtx();
		Ok(Decoded { samples: out, activity })
	}

	/// Native-rate frames one lost packet conceals, or `None` when this
	/// decoder cannot conceal one.
	pub fn conceal_len(&self) -> Option<usize> {
		match &self.backend {
			Backend::Opus(opus) => opus.last_frame_size,
			Backend::Pcm { .. } => None,
			#[cfg(feature = "aac")]
			Backend::Aac(_) => None,
		}
	}
}

impl Drop for Opus {
	fn drop(&mut self) {
		// SAFETY: `inner` is a live OpusDecoder that nothing else aliases.
		unsafe { opus_decoder_destroy(self.inner) };
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Three consecutive AAC-LC frames of a 440 Hz full-scale sine, mono at
	/// 44.1 kHz, and the AudioSpecificConfig that opens them. Generated with:
	///
	/// ```text
	/// ffmpeg -f lavfi -i "sine=frequency=440:sample_rate=44100:duration=0.2" -af volume=8 -ac 1 -c:a aac -b:a 32k -f adts sine.aac
	/// ```
	///
	/// then stripping the ADTS header off each frame, since the wire carries raw
	/// AAC. These are frames 2 to 4, past the encoder's priming. lavfi's sine is
	/// an eighth of full scale, which is what the volume filter is undoing.
	#[cfg(feature = "aac")]
	const AAC_DESCRIPTION: &[u8] = b"\x12\x08";

	#[cfg(feature = "aac")]
	const AAC_FRAMES: [&[u8]; 3] = [
		b"\x01\x52\xf2\x8b\x1a\xd7\x8e\x7b\xfd\xa7\xef\xe7\xe3\x55\xd3\x4d\x2f\x55\x2e\x47\x1c\x92\x49\x11\x20\x77\x3f\xbe\x74\xdd\x99\xb3\x7b\xfb\x90\xc9\xf0\x61\x9f\xdc\x0c\x9f\x06\x19\xfd\xe1\x1f\x1f\x00\x67\xf7\x03\x87\xc0\x19\xfd\xc0\xc9\xf0\x07",
		b"\x01\x1e\x32\x89\xe2\x9d\x6b\x33\xe7\xff\xe2\xfe\xbf\xfa\xff\xe7\x2f\x8b\xd5\xd5\xe7\x5f\x3f\x59\xeb\xf1\xcb\xba\xa5\x5e\x52\x4a\xbd\x8d\x74\x50\x8c\x08\xa8\xa0\xd4\x51\x40\xa1\x86\x5d\x06\xb4\x6c\x32\xe6\x25\x9a\x66\x75\xcd\xf9\xbf\x6f\x83\xb7\x53\x80",
		b"\x01\x1e\x32\x8a\x22\x7d\x40\x87\x48\xdb\xdf\xff\xf9\x4f\xff\x87\xde\xef\x8b\xeb\x1e\x77\x5d\xfc\x67\x8f\x8c\x77\x8a\xd6\x29\x96\x1f\x29\xe7\x39\xd4\x53\xcf\x3c\xf3\xce\x79\xd4\x27\x9c\xf5\x65\x2a\x9b\xe9\x80\xb7\xba\xa9\xf9\x58\xc7\x3c\x58\x27\x8a\x60\xa1\x57",
	];

	#[cfg(feature = "aac")]
	fn aac_catalog() -> hang::catalog::AudioConfig {
		let mut catalog = hang::catalog::AudioConfig::new(hang::catalog::AAC { profile: 2 }, 44_100, 1);
		catalog.description = Some(bytes::Bytes::from_static(AAC_DESCRIPTION));
		catalog
	}

	#[cfg(feature = "aac")]
	#[test]
	fn aac_decodes_a_sine() {
		let mut decoder = Decoder::new(&aac_catalog()).unwrap();
		assert_eq!(decoder.sample_rate(), 44_100);
		assert_eq!(decoder.channel_count(), 1);

		let decoded: Vec<Vec<f32>> = AAC_FRAMES
			.iter()
			.map(|frame| decoder.decode(frame).unwrap().samples)
			.collect();

		// AAC-LC frames are 1024 samples each, whatever the packet size.
		for pcm in &decoded {
			assert_eq!(pcm.len(), 1024);
		}

		// The first frame is missing the previous frame's overlap, so measure the
		// last one. ffmpeg decodes this same frame to 0.744 RMS, near the 0.707 of
		// an ideal full-scale sine.
		let last = decoded.last().unwrap();
		let rms = (last.iter().map(|s| s * s).sum::<f32>() / last.len() as f32).sqrt();
		assert!((0.65..0.8).contains(&rms), "expected a full-scale sine, got {rms} RMS");
	}

	#[cfg(feature = "aac")]
	#[test]
	fn aac_reports_a_truncated_packet_as_decode() {
		let mut decoder = Decoder::new(&aac_catalog()).unwrap();

		let truncated = &AAC_FRAMES[0][..16];
		assert!(matches!(decoder.decode(truncated), Err(Error::Decode(_))));
	}

	#[cfg(feature = "aac")]
	#[test]
	fn aac_synthesizes_a_missing_description() {
		// An MSF catalog carries the shape in its own fields instead.
		let mut catalog = aac_catalog();
		catalog.description = None;

		let mut decoder = Decoder::new(&catalog).unwrap();
		assert_eq!(decoder.sample_rate(), 44_100);
		assert_eq!(decoder.decode(AAC_FRAMES[0]).unwrap().samples.len(), 1024);
	}

	/// A packet libopus rejects is that packet's problem, not the
	/// configuration's. The distinction is what lets a consumer drop the frame and
	/// keep the subscription instead of ending the stream over one bad packet.
	#[test]
	fn opus_reports_a_rejected_packet_as_decode() {
		let head = moq_mux::codec::opus::Config::new(48_000, 2).encode().unwrap();
		let mut catalog = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
		catalog.description = Some(head);

		let mut decoder = Decoder::new(&catalog).unwrap();

		// Not a valid TOC byte sequence: libopus reports OPUS_INVALID_PACKET.
		assert!(matches!(decoder.decode(&[0xFF; 3]), Err(Error::Decode(_))));
	}

	#[test]
	fn pcm_rejects_incomplete_channel_frame() {
		let catalog = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Pcm, 48_000, 2);
		let mut decoder = Decoder::new(&catalog).unwrap();

		assert!(matches!(
			decoder.decode(&[]),
			Err(Error::Misaligned { got: 0, expected: 8 })
		));
		assert!(matches!(
			decoder.decode(&[0; 4]),
			Err(Error::Misaligned { got: 4, expected: 8 })
		));
	}

	#[test]
	fn decoder_rejects_unknown_codec() {
		let catalog = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Unknown("future".into()), 48_000, 2);

		assert!(matches!(Decoder::new(&catalog), Err(Error::Unsupported(_))));
	}

	#[test]
	fn pcm_rejects_incorrect_catalog_bitrate() {
		let mut catalog = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Pcm, 48_000, 2);
		catalog.bitrate = Some(1);

		assert!(matches!(Decoder::new(&catalog), Err(Error::Unsupported(_))));
	}

	/// Stepped-frequency sines: a lost frame's content cannot be guessed from
	/// its neighbors, so a recovered in-band copy must come measurably closer
	/// to the original than concealment does.
	fn stepped_sine_packets(fec: bool) -> Vec<bytes::Bytes> {
		use crate::encode::{Config as EncodeConfig, Encoder as EncodeEncoder, Input};

		let input = Input {
			format: Format::F32,
			sample_rate: 48_000,
			channels: 1,
		};
		let mut config = EncodeConfig::new(input);
		config.fec = fec;
		let mut encoder = EncodeEncoder::new(&config).unwrap();
		(1..=6)
			.map(|step| {
				let freq = 440.0 * step as f32;
				let pcm: Vec<f32> = (0..encoder.frame_size())
					.map(|n| (std::f32::consts::TAU * freq * n as f32 / 48_000.0).sin() * 0.5)
					.collect();
				encoder.encode(&pcm).unwrap().payload
			})
			.collect()
	}

	fn opus_catalog() -> hang::catalog::AudioConfig {
		let head = moq_mux::codec::opus::Config::new(48_000, 1).encode().unwrap();
		let mut catalog = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 1);
		catalog.description = Some(head);
		catalog
	}

	/// A lost packet comes back whole from the next packet's in-band copy,
	/// closer to the original than concealment manages.
	#[test]
	fn opus_recover_uses_the_in_band_copy_of_the_lost_frame() {
		let packets = stepped_sine_packets(true);
		let mut clean = Decoder::new(&opus_catalog()).unwrap();
		let decoded: Vec<Vec<f32>> = packets
			.iter()
			.map(|packet| clean.decode(packet).unwrap().samples)
			.collect();

		// Lose the fourth of six; the fifth carries its in-band copy.
		let mut recovered = Decoder::new(&opus_catalog()).unwrap();
		for packet in &packets[..3] {
			recovered.decode(packet).unwrap();
		}
		let fec = recovered.recover(Some(&packets[4])).unwrap().samples;

		let mut concealed = Decoder::new(&opus_catalog()).unwrap();
		for packet in &packets[..3] {
			concealed.decode(packet).unwrap();
		}
		let plc = concealed.recover(None).unwrap().samples;

		assert_eq!(fec.len(), decoded[3].len());
		let mse = |a: &[f32], b: &[f32]| {
			a.iter().zip(b).map(|(x, y)| (x - y).powi(2)).sum::<f32>() / a.len() as f32
		};
		assert!(
			mse(&fec, &decoded[3]) * 2.0 < mse(&plc, &decoded[3]),
			"the in-band copy should beat concealment against the original: fec={} plc={}",
			mse(&fec, &decoded[3]),
			mse(&plc, &decoded[3]),
		);
	}

	/// Without a next packet there is nothing to recover from: the loss
	/// conceals at the last packet's length, still active because the audio
	/// around it was.
	#[test]
	fn opus_recover_without_a_next_packet_conceals() {
		let packets = stepped_sine_packets(false);
		let mut decoder = Decoder::new(&opus_catalog()).unwrap();
		for packet in &packets[..3] {
			decoder.decode(packet).unwrap();
		}
		let concealed = decoder.recover(None).unwrap();
		assert_eq!(concealed.samples.len(), 960);
		assert!(!concealed.activity.is_dtx());
	}
}
