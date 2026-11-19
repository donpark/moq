//! LiquidCodec through the application-registered implementation (see [`crate::external`]).
//!
//! A [`Kind::Voice`](crate::encode::Kind::Voice) encode gates silence the way
//! Opus's discontinuous transmission does: after [`HANGOVER_FRAMES`] frames
//! below [`SILENCE_RMS`], frames come back as no-transmit packets (not coded at
//! all, which also spares the neural encoder), so the producer declares the end
//! of speech and sends nothing until the talker resumes. The frame speech
//! resumes with codes as the first of a stream, matching a decoder that starts
//! over after the declared break.

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;

use super::Backend;
use crate::encode::{Config, Encoded, Kind};
use crate::{Activity, Error};
use crate::external::{self, ExternalEncoder};

pub(super) const NAME: &str = "liquid";

/// Below this RMS (about -50 dBFS) a frame counts as silence.
pub(crate) const SILENCE_RMS: f32 = 0.003_16;
/// Silent frames still coded after speech, so word endings and short pauses
/// are kept: 4 frames, 320 ms.
pub(crate) const HANGOVER_FRAMES: usize = 4;

pub(super) struct Liquid {
	// The registered implementation is application code; a panic in it already
	// aborts the stream, so the encoder's unwind-safety promise is about our state.
	inner: AssertUnwindSafe<Box<dyn ExternalEncoder>>,
	/// Packets submitted but not yet received.
	ready: VecDeque<Encoded>,
	/// Silence gating (voice encodes only).
	gate: bool,
	/// Consecutive silent frames.
	silent_run: usize,
	/// Frames are being withheld; the next active one restarts the stream.
	in_dtx: bool,
}

impl Liquid {
	pub(super) fn open(config: &Config) -> Result<Box<dyn Backend>, Error> {
		let codec = external::liquid()
			.ok_or_else(|| Error::Unsupported("no liquid codec implementation is registered".into()))?;
		let inner = codec
			.encoder()
			.map_err(|err| Error::Unsupported(format!("liquid encoder: {err}")))?;
		Ok(Box::new(Self {
			inner: AssertUnwindSafe(inner),
			ready: VecDeque::new(),
			gate: config.kind == Some(Kind::Voice),
			silent_run: 0,
			in_dtx: false,
		}))
	}
}

impl Backend for Liquid {
	fn encode(&mut self, pcm: &[f32]) -> Result<(), Error> {
		if self.gate {
			let rms = (pcm.iter().map(|x| x * x).sum::<f32>() / pcm.len().max(1) as f32).sqrt();
			if rms < SILENCE_RMS {
				self.silent_run += 1;
			} else {
				self.silent_run = 0;
			}
			if self.silent_run > HANGOVER_FRAMES {
				self.in_dtx = true;
				let mut withheld = Encoded::new(bytes::Bytes::new());
				withheld.activity = Activity::Dtx;
				withheld.in_dtx = true;
				self.ready.push_back(withheld);
				return Ok(());
			}
			if std::mem::replace(&mut self.in_dtx, false) {
				// Speech resumes after a declared break: code it as a new stream.
				self.inner.reset();
			}
		}
		let packet = self
			.inner
			.encode(pcm)
			.map_err(|err| Error::Unsupported(format!("liquid encode: {err}")))?;
		self.ready.push_back(Encoded::new(packet.into()));
		Ok(())
	}

	fn receive(&mut self) -> Result<Option<Encoded>, Error> {
		Ok(self.ready.pop_front())
	}

	fn flush(&mut self) -> Result<(), Error> {
		Ok(())
	}

	fn reset(&mut self) {
		self.ready.clear();
		self.inner.reset();
		self.silent_run = 0;
		self.in_dtx = false;
	}

	fn set_bitrate(&mut self, _bitrate: u64) -> Result<(), Error> {
		Err(Error::Unsupported("liquid bitrate is fixed".into()))
	}

	fn bitrate(&self) -> u64 {
		self.inner.bitrate()
	}

	fn delay(&self) -> usize {
		external::liquid::ENCODER_DELAY
	}

	fn name(&self) -> &str {
		NAME
	}
}

#[cfg(test)]
mod tests {
	use std::sync::atomic::{AtomicUsize, Ordering};
	use std::sync::Arc;

	use super::*;
	use crate::external::{liquid as framing, ExternalCodec, ExternalDecoder};

	/// Counts encodes and resets; packets are one byte.
	#[derive(Default)]
	struct Counting {
		encodes: Arc<AtomicUsize>,
		resets: Arc<AtomicUsize>,
	}

	struct CountingEncoder {
		encodes: Arc<AtomicUsize>,
		resets: Arc<AtomicUsize>,
	}

	impl ExternalEncoder for CountingEncoder {
		fn encode(&mut self, _pcm: &[f32]) -> Result<Vec<u8>, String> {
			self.encodes.fetch_add(1, Ordering::Relaxed);
			Ok(vec![1])
		}
		fn reset(&mut self) {
			self.resets.fetch_add(1, Ordering::Relaxed);
		}
		fn bitrate(&self) -> u64 {
			3_400
		}
	}

	impl ExternalCodec for Counting {
		fn encoder(&self) -> Result<Box<dyn ExternalEncoder>, String> {
			Ok(Box::new(CountingEncoder { encodes: self.encodes.clone(), resets: self.resets.clone() }))
		}
		fn decoder(&self) -> Result<Box<dyn ExternalDecoder>, String> {
			Err("not used".into())
		}
	}


	fn run(kind: Option<Kind>, frames: &[f32]) -> (Vec<Activity>, usize, usize) {
		let codec = Arc::new(Counting::default());
		let (encodes, resets) = (codec.encodes.clone(), codec.resets.clone());
		external::register_liquid(Some(codec));
		let mut config = Config::from_input(crate::encode::Codec::Liquid, &crate::encode::Input::new(24_000, crate::Layout::Mono));
		config.kind = kind;
		let mut backend = Liquid::open(&config).unwrap();
		let mut activity = Vec::new();
		for &level in frames {
			backend.encode(&vec![level; framing::FRAME_SIZE]).unwrap();
			while let Some(packet) = backend.receive().unwrap() {
				activity.push(packet.activity);
			}
		}
		external::register_liquid(None);
		(activity, encodes.load(Ordering::Relaxed), resets.load(Ordering::Relaxed))
	}

	/// 5 speech frames, 10 silent, 3 speech.
	fn talk() -> Vec<f32> {
		[vec![0.2; 5], vec![0.0; 10], vec![0.2; 3]].concat()
	}

	#[test]
	fn voice_withholds_silence_after_the_hangover_and_restarts_on_speech() {
		let _guard = external::test_codec::LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
		let (activity, encodes, resets) = run(Some(Kind::Voice), &talk());
		let dtx: Vec<usize> = (0..activity.len()).filter(|&i| activity[i].is_dtx()).collect();
		// Speech (5) and the hangover (4) are coded; the other 6 silent frames are withheld.
		assert_eq!(dtx, (9..15).collect::<Vec<_>>(), "{activity:?}");
		assert_eq!(encodes, 5 + HANGOVER_FRAMES + 3, "withheld frames skip the encoder");
		assert_eq!(resets, 1, "speech after the break codes as a new stream");
	}

	#[test]
	fn other_kinds_code_every_frame() {
		let _guard = external::test_codec::LOCK.lock().unwrap_or_else(|poison| poison.into_inner());
		for kind in [None, Some(Kind::Music)] {
			let (activity, encodes, resets) = run(kind, &talk());
			assert!(activity.iter().all(|a| a.is_active()));
			assert_eq!((encodes, resets), (18, 0));
		}
	}
}
