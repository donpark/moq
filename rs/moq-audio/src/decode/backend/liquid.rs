//! LiquidCodec through the application-registered implementation (see [`crate::external`]).

use std::panic::AssertUnwindSafe;

use super::Backend;
use crate::decode::Decoded;
use crate::external::{self, ExternalDecoder};
use crate::{Activity, Error, Layout};

pub(super) const NAME: &str = "liquid";

pub(super) struct Liquid {
	inner: AssertUnwindSafe<Box<dyn ExternalDecoder>>,
}

impl Liquid {
	pub(super) fn open(catalog: &hang::catalog::AudioConfig) -> Result<Box<dyn Backend>, Error> {
		if catalog.sample_rate != external::liquid::SAMPLE_RATE || catalog.channel_count != 1 {
			return Err(Error::Unsupported(format!(
				"liquid is 24 kHz mono, not {} Hz x {}",
				catalog.sample_rate, catalog.channel_count
			)));
		}
		let codec = external::liquid()
			.ok_or_else(|| Error::Unsupported("no liquid codec implementation is registered".into()))?;
		let inner = codec
			.decoder()
			.map_err(|err| Error::Unsupported(format!("liquid decoder: {err}")))?;
		Ok(Box::new(Self {
			inner: AssertUnwindSafe(inner),
		}))
	}

	fn frame(samples: Result<Vec<f32>, String>) -> Result<Decoded, Error> {
		let samples = samples.map_err(|err| Error::Decode(format!("liquid: {err}")))?;
		if samples.len() != external::liquid::FRAME_SIZE {
			return Err(Error::Decode(format!(
				"liquid decoder returned {} samples, not {}",
				samples.len(),
				external::liquid::FRAME_SIZE
			)));
		}
		Ok(Decoded {
			samples,
			activity: Activity::Active,
		})
	}
}

impl Backend for Liquid {
	fn decode(&mut self, packet: &[u8]) -> Result<Decoded, Error> {
		// An empty packet marks one lost packet, as for Opus.
		if packet.is_empty() {
			return Self::frame(self.inner.recover(None, 0));
		}
		Self::frame(self.inner.decode(packet))
	}

	fn recover(&mut self, next: Option<&[u8]>, frames_to_next: usize) -> Result<Decoded, Error> {
		Self::frame(self.inner.recover(next, frames_to_next))
	}

	fn conceal_len(&self) -> Option<usize> {
		Some(external::liquid::FRAME_SIZE)
	}

	fn reset(&mut self) -> Result<(), Error> {
		self.inner.reset();
		Ok(())
	}

	fn sample_rate(&self) -> u32 {
		external::liquid::SAMPLE_RATE
	}

	fn layout(&self) -> Layout {
		Layout::Mono
	}

	fn delay(&self) -> usize {
		external::liquid::DECODER_DELAY
	}

	fn name(&self) -> &str {
		NAME
	}
}
