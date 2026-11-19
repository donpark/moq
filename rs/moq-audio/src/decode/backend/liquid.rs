//! LiquidCodec through the application-registered implementation (see [`crate::external`]).

use std::collections::VecDeque;
use std::panic::AssertUnwindSafe;

use super::Backend;
use crate::decode::Decoded;
use crate::external::{self, ExternalDecoder};
use crate::{Activity, Error, Layout};

pub(super) const NAME: &str = "liquid";

pub(super) struct Liquid {
	inner: AssertUnwindSafe<Box<dyn ExternalDecoder>>,
	/// Outputs submitted but not yet received.
	ready: VecDeque<Decoded>,
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
			ready: VecDeque::new(),
		}))
	}

	fn push(&mut self, samples: Result<Vec<f32>, String>) -> Result<(), Error> {
		let samples = samples.map_err(|err| Error::Decode(format!("liquid: {err}")))?;
		if samples.len() != external::liquid::FRAME_SIZE {
			return Err(Error::Decode(format!(
				"liquid decoder returned {} samples, not {}",
				samples.len(),
				external::liquid::FRAME_SIZE
			)));
		}
		self.ready.push_back(Decoded {
			samples,
			activity: Activity::Active,
		});
		Ok(())
	}
}

impl Backend for Liquid {
	fn decode(&mut self, packet: &[u8]) -> Result<(), Error> {
		// An empty packet marks one lost packet, as for Opus: the implementation
		// conceals one frame. Recovering from the next packet's redundant copy needs
		// a look-ahead the consumer does not give a backend.
		let samples = if packet.is_empty() {
			self.inner.recover(None, 0)
		} else {
			self.inner.decode(packet)
		};
		self.push(samples)
	}

	fn receive(&mut self) -> Result<Option<Decoded>, Error> {
		Ok(self.ready.pop_front())
	}

	fn flush(&mut self) -> Result<(), Error> {
		Ok(())
	}

	fn reset(&mut self) -> Result<(), Error> {
		self.ready.clear();
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
