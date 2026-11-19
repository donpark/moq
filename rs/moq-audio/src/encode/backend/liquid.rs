//! LiquidCodec through the application-registered implementation (see [`crate::external`]).

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;

use super::Backend;
use crate::Error;
use crate::encode::{Config, Encoded};
use crate::external::{self, ExternalEncoder};

pub(super) const NAME: &str = "liquid";

pub(super) struct Liquid {
	// The registered implementation is application code; a panic in it already
	// aborts the stream, so the encoder's unwind-safety promise is about our state.
	inner: AssertUnwindSafe<Box<dyn ExternalEncoder>>,
	/// Packets submitted but not yet received.
	ready: VecDeque<Encoded>,
}

impl Liquid {
	pub(super) fn open(_config: &Config) -> Result<Box<dyn Backend>, Error> {
		let codec = external::liquid()
			.ok_or_else(|| Error::Unsupported("no liquid codec implementation is registered".into()))?;
		let inner = codec
			.encoder()
			.map_err(|err| Error::Unsupported(format!("liquid encoder: {err}")))?;
		Ok(Box::new(Self {
			inner: AssertUnwindSafe(inner),
			ready: VecDeque::new(),
		}))
	}
}

impl Backend for Liquid {
	fn encode(&mut self, pcm: &[f32]) -> Result<(), Error> {
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
