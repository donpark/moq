//! Opus through libopus, the software decoder for every host.
//!
//! Family 0 (mono/stereo) decodes through the plain Opus decoder, which is
//! what DRED recovery needs: `opus_decoder_dred_decode` runs on the same
//! state, so a recovered hole keeps the prediction continuity. Family 1+
//! (surround) decodes through the multistream decoder, which has no DRED.

use opusic_sys::{
	OPUS_OK, OPUS_RESET_STATE, OPUS_SET_GAIN_REQUEST, OpusDRED, OpusDREDDecoder, OpusDecoder,
	OpusMSDecoder, opus_decode_float, opus_decoder_create, opus_decoder_ctl, opus_decoder_destroy,
	opus_decoder_dred_decode_float, opus_dred_alloc, opus_dred_decoder_create,
	opus_dred_decoder_destroy, opus_dred_free, opus_dred_parse, opus_multistream_decode_float,
	opus_multistream_decoder_create, opus_multistream_decoder_ctl, opus_multistream_decoder_destroy,
	opus_packet_has_lbrr,
};

use super::Backend;
use crate::decode::Decoded;
use crate::{Activity, Error, Layout, opus};

pub(super) const NAME: &str = "libopus";

/// Opus packets cap at 120 ms (RFC 6716 §2.1.4).
const MAX_FRAME_MS: usize = 120;

/// The family 0 streams: one stream, coupled when stereo, decoded in order.
const MONO: &[u8] = &[0];
const STEREO: &[u8] = &[0, 1];

/// The decoder behind [`Libopus`]: the plain one for family 0 (DRED-capable),
/// the multistream one for family 1+.
#[derive(Copy, Clone)]
enum Inner {
	One(*mut OpusDecoder),
	Many(*mut OpusMSDecoder),
}

/// The DRED parse state behind `recover`, cached per arriving packet so the
/// deep-redundancy decode runs once per hole instead of once per frame.
struct Dred {
	dec: *mut OpusDREDDecoder,
	state: *mut OpusDRED,
	/// The packet the cached parse came from.
	from: Vec<u8>,
	/// How deep the parse covered (samples before `from`'s start).
	available: usize,
}

pub(super) struct Libopus {
	inner: Inner,
	/// DRED parse state for `recover`, created on the first hole (family 0 only).
	dred: Option<Dred>,
	layout: Layout,
	/// For each canonical channel, the Vorbis-order channel it comes from, when they differ.
	reorder: Option<&'static [usize]>,
	/// Opus streams in one packet. Family 0 is one; the rest come from the mapping.
	streams: u8,
	pre_skip: usize,
	max_frame_size: usize,
	/// Frames per channel in the last packet that decoded, which is what a lost
	/// packet most likely carried. `None` until one has.
	last_frame_size: Option<usize>,
	in_dtx: bool,
}

// SAFETY: the decoder is owned exclusively and libopus keeps no thread-local state.
unsafe impl Send for Libopus {}

impl Libopus {
	/// Decodes at 48 kHz with the pre-skip and output gain the OpusHead
	/// `description` declares; its input rate is metadata. A missing description
	/// falls back to the catalog's channel count, which must then be mono or
	/// stereo. A description that does not parse is refused: guessing family 0
	/// would decode those packets with the wrong stream layout.
	///
	/// Channel mapping family 1 decodes up to 7.1 in the canonical [`Layout`]
	/// order; every other family is refused, since none declares speakers.
	pub(super) fn open(catalog: &hang::catalog::AudioConfig) -> Result<Box<dyn Backend>, Error> {
		let head = opus::head(catalog)?;
		let channel_count = head.channel_count;

		let (streams, coupled, table, layout, reorder) = match &head.mapping {
			None => {
				let table = match opus::validate_channels(channel_count)? {
					1 => MONO,
					_ => STEREO,
				};
				let layout = Layout::from_channels(channel_count)?;
				(1, table.len() as i32 - 1, table, layout, None)
			}
			Some(mapping) if mapping.family() == 1 => {
				let (layout, reorder) = vorbis(channel_count)?;
				let (streams, coupled) = (mapping.streams() as i32, mapping.coupled() as i32);
				(streams, coupled, mapping.table(), layout, reorder)
			}
			Some(mapping) => {
				return Err(Error::Unsupported(format!(
					"opus channel mapping family {} declares no speaker positions",
					mapping.family()
				)));
			}
		};

		let mut err = 0i32;
		let inner = if head.mapping.is_none() {
			// Family 0: the plain decoder, so DRED recovery runs on this state.
			// SAFETY: the out-pointer is valid; inner is checked for null below.
			let inner = unsafe { opus_decoder_create(opus::DECODE_RATE as i32, channel_count as i32, &mut err) };
			if err != OPUS_OK || inner.is_null() {
				return Err(opus::error(err, "opus_decoder_create"));
			}
			Inner::One(inner)
		} else {
			// SAFETY: `table` holds one entry per output channel, the count we pass,
			// and the out-pointer is valid; inner is checked for null below.
			let inner = unsafe {
				opus_multistream_decoder_create(
					opus::DECODE_RATE as i32,
					table.len() as i32,
					streams,
					coupled,
					table.as_ptr(),
					&mut err,
				)
			};
			if err != OPUS_OK || inner.is_null() {
				return Err(opus::error(err, "opus_multistream_decoder_create"));
			}
			Inner::Many(inner)
		};

		// Owned before the gain ctl so a failure there still destroys the decoder.
		let decoder = Self {
			inner,
			dred: None,
			layout,
			reorder,
			streams: streams as u8,
			pre_skip: head.pre_skip as usize,
			max_frame_size: (opus::DECODE_RATE as usize * MAX_FRAME_MS) / 1000,
			last_frame_size: None,
			in_dtx: false,
		};

		// Same Q7.8 dB units as OpusHead, and it survives OPUS_RESET_STATE.
		// SAFETY: `inner` owns a live decoder and OPUS_SET_GAIN takes one i32.
		let rc = unsafe {
			match decoder.inner {
				Inner::One(dec) => opus_decoder_ctl(dec, OPUS_SET_GAIN_REQUEST, head.output_gain as i32),
				Inner::Many(dec) => {
					opus_multistream_decoder_ctl(dec, OPUS_SET_GAIN_REQUEST, head.output_gain as i32)
				}
			}
		};
		if rc != OPUS_OK {
			return Err(opus::error(rc, "OPUS_SET_GAIN"));
		}

		Ok(Box::new(decoder))
	}

	/// One decode through whichever decoder `inner` owns. `fec` asks for the
	/// frame before `packet` from its in-band copy instead (the recovery path).
	///
	/// # Safety
	///
	/// The decoder is live and exclusively owned; `out` holds
	/// `frame_size * channels` samples.
	unsafe fn decode_raw(&self, packet: &[u8], frame_size: usize, out: &mut [f32], fec: i32) -> i32 {
		unsafe {
			match self.inner {
				Inner::One(dec) => opus_decode_float(
					dec,
					packet.as_ptr(),
					packet.len() as i32,
					out.as_mut_ptr(),
					frame_size as i32,
					fec,
				),
				Inner::Many(dec) => opus_multistream_decode_float(
					dec,
					packet.as_ptr(),
					packet.len() as i32,
					out.as_mut_ptr(),
					frame_size as i32,
					fec,
				),
			}
		}
	}

	fn activity(&self, packet: &[u8]) -> Activity {
		match self.inner {
			Inner::One(_) => opus::activity(packet, self.in_dtx),
			Inner::Many(_) => opus::multistream_activity(packet, self.streams, self.in_dtx),
		}
	}

	/// The deep redundancy `packet` carries for the frame `frames_to_next`
	/// frames before it, decoded into `out`, or `None` when the packet carries
	/// none that deep. The DNN parse runs once per `next` packet and is cached
	/// across the frames of one hole.
	fn dred_recover(
		&mut self,
		packet: &[u8],
		frames_to_next: usize,
		frame_size: usize,
		out: &mut [f32],
	) -> Result<Option<i32>, Error> {
		let offset = frames_to_next * frame_size;
		if offset == 0 {
			return Ok(None);
		}
		// Family 0 only: `opus_decoder_dred_decode` takes the plain decoder.
		let Inner::One(dec) = self.inner else {
			return Ok(None);
		};
		if self.dred.is_none() {
			let mut err = 0i32;
			// SAFETY: the out-pointer is valid; the states are checked below.
			let dred_dec = unsafe { opus_dred_decoder_create(&mut err) };
			if err != OPUS_OK || dred_dec.is_null() {
				return Err(opus::error(err, "opus_dred_decoder_create"));
			}
			// SAFETY: the out-pointer is valid; the state is checked below.
			let state = unsafe { opus_dred_alloc(&mut err) };
			if err != OPUS_OK || state.is_null() {
				// SAFETY: dred_dec is a live DRED decoder we own.
				unsafe { opus_dred_decoder_destroy(dred_dec) };
				return Err(opus::error(err, "opus_dred_alloc"));
			}
			self.dred = Some(Dred {
				dec: dred_dec,
				state,
				from: Vec::new(),
				available: 0,
			});
		}
		let dred = self.dred.as_mut().unwrap();
		if dred.from != packet {
			let mut dred_end = 0i32;
			// SAFETY: the DRED states are live and exclusively owned; the packet
			// slice is bounded by the length we pass. The work hint is one second:
			// the parse decodes latents in pairs sized off it, so asking for the
			// exact depth needed decodes only about half of it.
			let ret = unsafe {
				opus_dred_parse(
					dred.dec,
					dred.state,
					packet.as_ptr(),
					packet.len() as i32,
					opus::DECODE_RATE as i32,
					opus::DECODE_RATE as i32,
					&mut dred_end,
					0,
				)
			};
			if ret < 0 {
				return Err(opus::decode_error(ret));
			}
			dred.from = packet.to_vec();
			dred.available = ret as usize;
		}
		let dred = self.dred.as_ref().unwrap();
		if offset > dred.available {
			return Ok(None);
		}
		// SAFETY: the decoder and DRED state are live; `out` holds
		// `frame_size * channels` samples.
		let samples = unsafe {
			opus_decoder_dred_decode_float(dec, dred.state, offset as i32, out.as_mut_ptr(), frame_size as i32)
		};
		if samples < 0 {
			return Err(opus::decode_error(samples));
		}
		Ok(Some(samples))
	}
}

/// The layout a family 1 stream of `channels` declares (RFC 7845 §5.1.1.2), and
/// how to reorder its Vorbis channel order into that layout's canonical one.
///
/// Vorbis puts the center between the fronts and the LFE last. Its "rear"
/// pair in 5.0 and 5.1 is the surround pair, which is side in the canonical
/// layouts, while 7.1 has distinct side and rear pairs.
fn vorbis(channels: u32) -> Result<(Layout, Option<&'static [usize]>), Error> {
	Ok(match channels {
		1 => (Layout::Mono, None),
		2 => (Layout::Stereo, None),
		// L, C, R.
		3 => (Layout::ThreePointZero, Some(&[0, 2, 1])),
		// FL, FR, RL, RR.
		4 => (Layout::Quad, None),
		// FL, C, FR, RL, RR.
		5 => (Layout::FivePointZero, Some(&[0, 2, 1, 3, 4])),
		// FL, C, FR, RL, RR, LFE.
		6 => (Layout::FivePointOne, Some(&[0, 2, 1, 5, 3, 4])),
		// FL, C, FR, SL, SR, RC, LFE.
		7 => (Layout::SixPointOne, Some(&[0, 2, 1, 6, 5, 3, 4])),
		// FL, C, FR, SL, SR, RL, RR, LFE.
		8 => (Layout::SevenPointOne, Some(&[0, 2, 1, 7, 5, 6, 3, 4])),
		other => {
			return Err(Error::Unsupported(format!(
				"opus channel mapping family 1 has no {other}-channel layout"
			)));
		}
	})
}

impl Backend for Libopus {
	/// An empty packet conceals as much audio as the last packet that decoded
	/// held, and is refused before any has. Loss during DTX remains classified
	/// as DTX, while loss during active audio remains active.
	fn decode(&mut self, packet: &[u8]) -> Result<Decoded, Error> {
		let channels = self.layout.channels() as usize;
		// libopus conceals exactly `frame_size` samples for a lost packet, and
		// the decoder can't know how long it was, so assume the last one's length.
		// A real packet decodes into the most it can hold.
		let frame_size = if packet.is_empty() {
			self.last_frame_size.ok_or_else(|| {
				Error::Decode("opus packet lost before any packet decoded, so no length to conceal".into())
			})?
		} else {
			self.max_frame_size
		};
		let mut out = vec![0.0f32; frame_size * channels];
		// SAFETY: `inner` owns a live decoder; packet/out slices are bounded by
		// the lengths we pass.
		let samples = unsafe { self.decode_raw(packet, frame_size, &mut out, 0) };
		if samples < 0 {
			return Err(opus::decode_error(samples));
		}
		if !packet.is_empty() {
			self.last_frame_size = Some(samples as usize);
		}
		out.truncate(samples as usize * channels);

		if let Some(order) = self.reorder {
			let mut vorbis = [0.0f32; 8];
			for frame in out.chunks_exact_mut(channels) {
				vorbis[..channels].copy_from_slice(frame);
				for (sample, &from) in frame.iter_mut().zip(order) {
					*sample = vorbis[from];
				}
			}
		}

		let activity = self.activity(packet);
		self.in_dtx = activity.is_dtx();
		Ok(Decoded { samples: out, activity })
	}

	/// Recover one lost packet, `frames_to_next` frames before the arriving
	/// packet `next` carries. Sources, best first (the `opus_demo` recovery
	/// order): the packet's in-band forward error copy for the frame right
	/// before it, then its deep redundancy (DRED) at that offset, then
	/// concealment. `next` is `None` (or empty) for concealment only.
	///
	/// The copy and the DRED decode both run on this decoder, so recovery
	/// advances it as if the frame had decoded. The recovered length is the
	/// last packet's, and the copy decodes at the reduced rate the sender
	/// coded it at. Classified as the loss it covers rather than as `next`,
	/// whose own activity says nothing about the frames before it.
	fn recover(&mut self, next: Option<&[u8]>, frames_to_next: usize) -> Result<Decoded, Error> {
		let channels = self.layout.channels() as usize;
		let frame_size = self.last_frame_size.ok_or_else(|| {
			Error::Decode("opus packet lost before any packet decoded, so no length to conceal".into())
		})?;
		let next = next.filter(|packet| !packet.is_empty());
		let mut out = vec![0.0f32; frame_size * channels];
		let samples = match next {
			Some(packet)
				if frames_to_next == 1
					&& unsafe { opus_packet_has_lbrr(packet.as_ptr(), packet.len() as i32) } != 0 =>
			{
				unsafe { self.decode_raw(packet, frame_size, &mut out, 1) }
			}
			Some(packet) => {
				if let Some(samples) = self.dred_recover(packet, frames_to_next, frame_size, &mut out)? {
					samples
				} else if frames_to_next == 1 {
					// No DRED: the FEC path conceals when the copy is absent.
					unsafe { self.decode_raw(packet, frame_size, &mut out, 1) }
				} else {
					unsafe { self.decode_raw(&[], frame_size, &mut out, 0) }
				}
			}
			None => unsafe { self.decode_raw(&[], frame_size, &mut out, 0) },
		};
		if samples < 0 {
			return Err(opus::decode_error(samples));
		}
		out.truncate(samples as usize * channels);
		if let Some(order) = self.reorder {
			let mut vorbis = [0.0f32; 8];
			for frame in out.chunks_exact_mut(channels) {
				vorbis[..channels].copy_from_slice(frame);
				for (sample, &from) in frame.iter_mut().zip(order) {
					*sample = vorbis[from];
				}
			}
		}
		let activity = self.activity(&[]);
		self.in_dtx = activity.is_dtx();
		Ok(Decoded { samples: out, activity })
	}

	fn conceal_len(&self) -> Option<usize> {
		self.last_frame_size
	}

	fn reset(&mut self) -> Result<(), Error> {
		// SAFETY: `inner` owns a live decoder and OPUS_RESET_STATE takes no arguments.
		let rc = unsafe {
			match self.inner {
				Inner::One(dec) => opus_decoder_ctl(dec, OPUS_RESET_STATE),
				Inner::Many(dec) => opus_multistream_decoder_ctl(dec, OPUS_RESET_STATE),
			}
		};
		if rc != OPUS_OK {
			return Err(opus::error(rc, "OPUS_RESET_STATE"));
		}
		self.in_dtx = false;
		Ok(())
	}

	fn sample_rate(&self) -> u32 {
		opus::DECODE_RATE
	}

	fn layout(&self) -> Layout {
		self.layout
	}

	fn delay(&self) -> usize {
		self.pre_skip
	}

	fn name(&self) -> &str {
		NAME
	}
}

impl Drop for Libopus {
	fn drop(&mut self) {
		// SAFETY: each pointer is a live libopus object we own exclusively.
		unsafe {
			match self.inner {
				Inner::One(dec) => opus_decoder_destroy(dec),
				Inner::Many(dec) => opus_multistream_decoder_destroy(dec),
			}
			if let Some(dred) = self.dred.take() {
				opus_dred_free(dred.state);
				opus_dred_decoder_destroy(dred.dec);
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use opusic_sys::{
		OPUS_APPLICATION_AUDIO, opus_multistream_encode_float, opus_multistream_encoder_destroy,
		opus_multistream_surround_encoder_create,
	};

	use crate::Error;
	use crate::decode::{Config, Decoder};
	use crate::layout::Speaker::{self, *};

	/// Speakers in Vorbis channel order for each family 1 channel count (RFC 7845 §5.1.1.2).
	const VORBIS: [&[Speaker]; 8] = [
		&[FrontCenter],
		&[FrontLeft, FrontRight],
		&[FrontLeft, FrontCenter, FrontRight],
		&[FrontLeft, FrontRight, BackLeft, BackRight],
		&[FrontLeft, FrontCenter, FrontRight, SideLeft, SideRight],
		&[FrontLeft, FrontCenter, FrontRight, SideLeft, SideRight, Lfe],
		&[FrontLeft, FrontCenter, FrontRight, SideLeft, SideRight, BackCenter, Lfe],
		&[
			FrontLeft,
			FrontCenter,
			FrontRight,
			SideLeft,
			SideRight,
			BackLeft,
			BackRight,
			Lfe,
		],
	];

	const RATE: usize = 48_000;
	const FRAME: usize = 960;
	const PACKETS: usize = 15;

	/// A distinct tone per speaker, low for the LFE, which libopus band-limits.
	fn tone(speaker: Speaker) -> f32 {
		match speaker {
			Lfe => 80.0,
			other => 400.0 + 300.0 * other as u8 as f32,
		}
	}

	/// Encode `channels` of Vorbis-ordered tones as a family 1 stream, returning
	/// its OpusHead and packets.
	fn surround(channels: usize) -> (bytes::Bytes, Vec<Vec<u8>>) {
		let speakers = VORBIS[channels - 1];
		let (mut streams, mut coupled, mut mapping) = (0i32, 0i32, [0u8; 8]);
		let mut err = 0i32;
		// SAFETY: every out-pointer is valid and `mapping` holds `channels` entries.
		let encoder = unsafe {
			opus_multistream_surround_encoder_create(
				RATE as i32,
				channels as i32,
				1,
				&mut streams,
				&mut coupled,
				mapping.as_mut_ptr(),
				OPUS_APPLICATION_AUDIO,
				&mut err,
			)
		};
		assert!(err == 0 && !encoder.is_null(), "encoder create failed: {err}");

		let packets = (0..PACKETS)
			.map(|packet| {
				let mut pcm = Vec::with_capacity(FRAME * channels);
				for i in packet * FRAME..(packet + 1) * FRAME {
					for &speaker in speakers {
						let phase = std::f32::consts::TAU * tone(speaker) * i as f32 / RATE as f32;
						pcm.push(phase.sin() * 0.5);
					}
				}
				let mut out = vec![0u8; 4000];
				// SAFETY: `encoder` is live, `pcm` holds FRAME frames, and `out` is as long as we say.
				let len = unsafe {
					opus_multistream_encode_float(encoder, pcm.as_ptr(), FRAME as i32, out.as_mut_ptr(), 4000)
				};
				assert!(len > 0, "encode failed: {len}");
				out.truncate(len as usize);
				out
			})
			.collect();
		// SAFETY: `encoder` is live and not used again.
		unsafe { opus_multistream_encoder_destroy(encoder) };

		let mut head = moq_mux::codec::opus::Config::new(RATE as u32, 2)
			.encode()
			.unwrap()
			.to_vec();
		head[9] = channels as u8;
		head[18] = 1;
		head.extend_from_slice(&[streams as u8, coupled as u8]);
		head.extend_from_slice(&mapping[..channels]);
		(head.into(), packets)
	}

	fn catalog(head: bytes::Bytes, channels: u32) -> hang::catalog::AudioConfig {
		let mut catalog = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, channels);
		catalog.description = Some(head);
		catalog
	}

	/// Energy of `samples` at `freq`, by the Goertzel recurrence.
	fn power(samples: &[f32], freq: f32) -> f32 {
		let coeff = 2.0 * (std::f32::consts::TAU * freq / RATE as f32).cos();
		let (mut s1, mut s2) = (0.0f32, 0.0f32);
		for &x in samples {
			let s = x + coeff * s1 - s2;
			s2 = s1;
			s1 = s;
		}
		s1 * s1 + s2 * s2 - coeff * s1 * s2
	}

	/// Every family 1 layout decodes with each canonical channel carrying its
	/// own speaker's tone, which pins the Vorbis reorder independently of it.
	#[test]
	fn family_one_decodes_in_canonical_order() {
		for channels in 1..=8 {
			let (head, packets) = surround(channels);
			let mut decoder = Decoder::new(&catalog(head, channels as u32), &Config::default()).unwrap();
			let layout = decoder.layout();
			// The layout has exactly the Vorbis speakers, which canonical order sorts.
			let mut speakers = VORBIS[channels - 1].to_vec();
			speakers.sort_by_key(|&speaker| speaker as u8);
			assert_eq!(layout.speakers().unwrap(), speakers, "{channels} channels");

			let mut pcm = Vec::new();
			for packet in &packets {
				pcm.extend(decoder.decode(packet).unwrap().samples);
			}
			// Skip the encoder's warmup.
			let pcm = &pcm[pcm.len() / 2..];

			let tones: Vec<f32> = VORBIS[channels - 1].iter().copied().map(tone).collect();
			for (index, &speaker) in layout.speakers().unwrap().iter().enumerate() {
				let channel: Vec<f32> = pcm.iter().skip(index).step_by(channels).copied().collect();
				let loudest = tones
					.iter()
					.copied()
					.max_by(|a, b| power(&channel, *a).total_cmp(&power(&channel, *b)))
					.unwrap();
				assert_eq!(loudest, tone(speaker), "{channels} channels, {speaker:?} at {index}");
			}
		}
	}

	/// An all-DTX surround packet is one empty Opus packet per stream. Read as a
	/// single stream, the later subpackets look like payload and the span is lost.
	#[test]
	fn surround_silence_is_dtx() {
		let (head, packets) = surround(6);
		let mut decoder = Decoder::new(&catalog(head.clone(), 6), &Config::default()).unwrap();
		let mid = decoder.decode(&packets[PACKETS / 2]).unwrap();
		assert!(mid.activity.is_active(), "a coded surround frame must stay active");

		// Two coupled streams, then two mono, each a 20 ms empty frame.
		let dtx = [0xfc, 0x00, 0xfc, 0x00, 0xf8, 0x00, 0xf8];
		let mut decoder = Decoder::new(&catalog(head, 6), &Config::default()).unwrap();
		let decoded = decoder.decode(&dtx).expect("empty multistream packet");
		assert!(decoded.activity.is_dtx(), "all-DTX surround packet read as active");
		assert_eq!(decoded.samples.len() % 6, 0);
	}

	/// A description that is present but truncated used to be dropped, and a
	/// stereo catalog then opened a family 0 decoder for a family 1 or 255 head.
	#[test]
	fn malformed_description_is_refused() {
		for family in [1u8, 255] {
			let mut head = moq_mux::codec::opus::Config::new(48_000, 2).encode().unwrap().to_vec();
			head[18] = family;
			let err = Decoder::new(&catalog(head.into(), 2), &Config::default())
				.err()
				.expect("refused");
			assert!(
				matches!(&err, Error::Unsupported(message) if message.contains("opus description")),
				"family {family}: {err}"
			);
		}

		let plain = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Opus, 48_000, 2);
		assert!(Decoder::new(&plain, &Config::default()).is_ok());
	}

	/// Families other than 0 and 1 carry no speaker positions, so there is
	/// nothing to put in a layout: refused, not passed through as discrete.
	#[test]
	fn other_families_are_refused() {
		for (family, channels, table) in [(255u8, 2u8, &[2, 0, 0, 1][..]), (2, 4, &[4, 0, 0, 1, 2, 3])] {
			let mut head = moq_mux::codec::opus::Config::new(48_000, 2).encode().unwrap().to_vec();
			head[9] = channels;
			head[18] = family;
			head.extend_from_slice(table);

			let err = Decoder::new(&catalog(head.into(), channels.into()), &Config::default())
				.err()
				.expect("refused");
			assert!(
				matches!(&err, Error::Unsupported(message) if message.contains("no speaker positions")),
				"family {family}: {err}"
			);
		}
	}
}
