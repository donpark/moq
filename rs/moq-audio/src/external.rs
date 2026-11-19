//! Codecs whose implementation the application registers at runtime.
//!
//! [`Codec::Liquid`](crate::encode::Codec::Liquid) is a neural speech codec that
//! runs on platform accelerators (Core ML on Apple devices), so this crate does
//! not link an implementation. The application registers one with
//! [`register_liquid`]; until it does, the encoder and decoder backends refuse
//! the codec at construction, so a host without one never advertises it.
//!
//! The contract is one packet per frame in both directions: an encoder takes
//! exactly [`liquid::FRAME_SIZE`] mono samples at [`liquid::SAMPLE_RATE`] and
//! returns one packet; a decoder turns one packet back into exactly
//! [`liquid::FRAME_SIZE`] samples. Packet bytes are opaque here.

use std::sync::{Arc, RwLock};

/// LiquidCodec framing constants.
pub mod liquid {
	/// The only rate the codec runs at.
	pub const SAMPLE_RATE: u32 = 24_000;
	/// Samples per 80 ms frame.
	pub const FRAME_SIZE: usize = 1920;
	/// Encoder look-ahead in samples: each packet codes audio delayed by this
	/// much, which the producer folds into its timestamps.
	pub const ENCODER_DELAY: usize = 480;
	/// Decoder priming in samples, trimmed from the start of the decoded stream.
	pub const DECODER_DELAY: usize = 480;
}

/// One encoder stream.
pub trait ExternalEncoder: Send {
	/// Encode exactly one frame of mono `f32` samples into one packet.
	fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, String>;
	/// Forget history so the next frame codes as the first of a stream.
	fn reset(&mut self);
	/// The bitrate the packets average, in bits per second.
	fn bitrate(&self) -> u64;
}

/// One decoder stream.
pub trait ExternalDecoder: Send {
	/// Decode one packet into exactly one frame of mono `f32` samples.
	fn decode(&mut self, packet: &[u8]) -> Result<Vec<f32>, String>;
	/// Produce one frame for a lost packet, `frames_to_next` frames before the
	/// packet `next` (when one has arrived), which may carry a redundant copy.
	fn recover(&mut self, next: Option<&[u8]>, frames_to_next: usize) -> Result<Vec<f32>, String>;
	/// Forget history after a discontinuity.
	fn reset(&mut self);
}

/// A registered codec implementation: a factory for encoder and decoder streams.
pub trait ExternalCodec: Send + Sync {
	/// Open an encoder stream.
	fn encoder(&self) -> Result<Box<dyn ExternalEncoder>, String>;
	/// Open a decoder stream.
	fn decoder(&self) -> Result<Box<dyn ExternalDecoder>, String>;
}

static LIQUID: RwLock<Option<Arc<dyn ExternalCodec>>> = RwLock::new(None);

/// Install (or with `None`, remove) the LiquidCodec implementation. Streams
/// already open keep the implementation they opened with.
pub fn register_liquid(codec: Option<Arc<dyn ExternalCodec>>) {
	*LIQUID.write().unwrap_or_else(|poison| poison.into_inner()) = codec;
}

/// The registered LiquidCodec implementation, if any.
pub fn liquid() -> Option<Arc<dyn ExternalCodec>> {
	LIQUID.read().unwrap_or_else(|poison| poison.into_inner()).clone()
}

/// Whether a LiquidCodec implementation is registered.
pub fn liquid_available() -> bool {
	liquid().is_some()
}

#[cfg(test)]
pub(crate) mod test_codec {
	//! A deterministic stand-in: a packet is the frame's mean as 4 bytes, and
	//! decoding yields a constant frame of that value.

	use super::*;

	pub struct Mean;
	struct Enc;
	struct Dec {
		last: f32,
	}

	impl ExternalEncoder for Enc {
		fn encode(&mut self, pcm: &[f32]) -> Result<Vec<u8>, String> {
			assert_eq!(pcm.len(), liquid::FRAME_SIZE);
			let mean = pcm.iter().sum::<f32>() / pcm.len() as f32;
			Ok(mean.to_le_bytes().to_vec())
		}
		fn reset(&mut self) {}
		fn bitrate(&self) -> u64 {
			400
		}
	}

	impl ExternalDecoder for Dec {
		fn decode(&mut self, packet: &[u8]) -> Result<Vec<f32>, String> {
			let bytes: [u8; 4] = packet.try_into().map_err(|_| "bad packet".to_string())?;
			self.last = f32::from_le_bytes(bytes);
			Ok(vec![self.last; liquid::FRAME_SIZE])
		}
		fn recover(&mut self, _next: Option<&[u8]>, _frames_to_next: usize) -> Result<Vec<f32>, String> {
			Ok(vec![self.last; liquid::FRAME_SIZE])
		}
		fn reset(&mut self) {
			self.last = 0.0;
		}
	}

	impl ExternalCodec for Mean {
		fn encoder(&self) -> Result<Box<dyn ExternalEncoder>, String> {
			Ok(Box::new(Enc))
		}
		fn decoder(&self) -> Result<Box<dyn ExternalDecoder>, String> {
			Ok(Box::new(Dec { last: 0.0 }))
		}
	}

	/// The registry is process-global; tests that touch it hold this lock.
	pub static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::decode::{Config as DecodeConfig, Decoder};
	use crate::encode::{Codec, Config, Encoder, Input};
	use crate::{Error, Layout};

	fn config() -> Config {
		Config::from_input(Codec::Liquid, &Input::new(48_000, Layout::Stereo))
	}

	fn one(dec: &mut Decoder, packet: &[u8]) -> Vec<f32> {
		let mut out = dec.decode_all(packet).unwrap();
		assert_eq!(out.len(), 1);
		out.remove(0).samples
	}

	#[test]
	fn liquid_is_refused_without_an_implementation() {
		let _guard = test_codec::LOCK.lock().unwrap();
		register_liquid(None);
		assert!(matches!(Encoder::new(&config()), Err(Error::Unsupported(_))));
		let catalog = hang::catalog::AudioConfig::new(hang::catalog::AudioCodec::Liquid, 24_000, 1);
		assert!(matches!(
			Decoder::new(&catalog, &DecodeConfig::default()),
			Err(Error::Unsupported(_))
		));
	}

	#[test]
	fn liquid_config_follows_the_codec() {
		let c = config();
		assert_eq!((c.sample_rate, c.layout), (liquid::SAMPLE_RATE, Layout::Mono));
		assert_eq!(c.frame_duration, std::time::Duration::from_millis(80));
		let _guard = test_codec::LOCK.lock().unwrap();
		register_liquid(Some(Arc::new(test_codec::Mean)));
		let mut fixed = config();
		fixed.bitrate = Some(moq_net::bandwidth::Rate::from_bps(3_400));
		assert!(matches!(Encoder::new(&fixed), Err(Error::Unsupported(_))));
		let mut short = config();
		short.frame_duration = std::time::Duration::from_millis(20);
		assert!(matches!(Encoder::new(&short), Err(Error::Unsupported(_))));
		register_liquid(None);
	}

	#[test]
	fn liquid_round_trips_through_the_registered_codec() {
		let _guard = test_codec::LOCK.lock().unwrap();
		register_liquid(Some(Arc::new(test_codec::Mean)));
		let mut enc = Encoder::new(&config()).unwrap();
		assert_eq!(enc.frame_size(), liquid::FRAME_SIZE);
		assert_eq!(enc.folded_delay(), liquid::ENCODER_DELAY);
		let catalog = enc.catalog();
		assert_eq!(catalog.codec, hang::catalog::AudioCodec::Liquid);
		assert_eq!(
			(catalog.sample_rate, catalog.channel_count, catalog.bitrate),
			(24_000, 1, Some(400))
		);

		let mut dec = Decoder::new(&catalog, &DecodeConfig::default()).unwrap();
		assert_eq!(
			(dec.name(), dec.sample_rate(), dec.layout()),
			("liquid", 24_000, Layout::Mono)
		);
		let packet = enc.encode_all(&vec![0.25; liquid::FRAME_SIZE]).unwrap();
		assert_eq!(packet.len(), 1);
		let first = one(&mut dec, &packet[0].payload);
		assert_eq!(first.len(), liquid::FRAME_SIZE - liquid::DECODER_DELAY);
		assert!(first.iter().all(|&s| s == 0.25));
		let packet = enc.encode_all(&vec![-0.5; liquid::FRAME_SIZE]).unwrap();
		let second = one(&mut dec, &packet[0].payload);
		assert_eq!(second.len(), liquid::FRAME_SIZE);
		// a lost packet conceals one full frame
		assert_eq!(one(&mut dec, &[]), vec![-0.5; liquid::FRAME_SIZE]);
		register_liquid(None);
	}
}
