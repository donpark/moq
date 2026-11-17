//! LiquidCodec through the application-registered implementation (see [`crate::external`]).

use std::panic::AssertUnwindSafe;

use super::Backend;
use crate::Error;
use crate::encode::{Encoded, Settings};
use crate::external::{self, ExternalEncoder};

pub(super) const NAME: &str = "liquid";

pub(super) struct Liquid {
	// The registered implementation is application code; a panic in it already
	// aborts the stream, so the encoder's unwind-safety promise is about our state.
	inner: AssertUnwindSafe<Box<dyn ExternalEncoder>>,
}

impl Liquid {
	pub(super) fn open(_settings: &Settings) -> Result<Box<dyn Backend>, Error> {
		let codec = external::liquid()
			.ok_or_else(|| Error::Unsupported("no liquid codec implementation is registered".into()))?;
		let inner = codec
			.encoder()
			.map_err(|err| Error::Unsupported(format!("liquid encoder: {err}")))?;
		Ok(Box::new(Self {
			inner: AssertUnwindSafe(inner),
		}))
	}
}

impl Backend for Liquid {
	fn encode(&mut self, pcm: &[f32]) -> Result<Encoded, Error> {
		let packet = self
			.inner
			.encode(pcm)
			.map_err(|err| Error::Unsupported(format!("liquid encode: {err}")))?;
		Ok(Encoded::new(packet.into()))
	}

	fn reset(&mut self) {
		self.inner.reset();
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
