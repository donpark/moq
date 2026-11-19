//! Loss recovery for codecs that rebuild a lost packet from the packet after it.

use std::time::Duration;

/// How a codec recovers lost packets itself, as [`Decoder::recovery`](super::Decoder::recovery)
/// reports it.
///
/// A codec that carries redundancy in later packets (LiquidCodec's copy of the previous
/// frame, Opus's in-band FEC and deep redundancy) can rebuild a lost packet once the packet
/// after it arrives. A [`Consumer`](super::Consumer) that recovers holds at least
/// [`lookahead`](Self::lookahead) of playout delay so the next packet arrives in time, and
/// bridges a hole up to [`max_burst`](Self::max_burst) long without a [`Read::Gap`](super::Read::Gap).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct Recovery {
	/// Playout delay the codec needs to see the packet after a lost one before the lost one
	/// plays: one packet's duration for single-frame redundancy.
	pub lookahead: Duration,
	/// The longest hole bridged by recovery and concealment. A longer one is a gap.
	pub max_burst: Duration,
	/// Whether a [`Consumer`](super::Consumer) recovers by default, when its
	/// [`Options::recover`](super::Options::recover) leaves the choice to the codec.
	pub by_default: bool,
}

impl Recovery {
	/// Recovery with `lookahead` of playout delay, bridging holes up to `max_burst`.
	pub fn new(lookahead: Duration, max_burst: Duration, by_default: bool) -> Self {
		Self {
			lookahead,
			max_burst,
			by_default,
		}
	}
}

/// Frames a [`Consumer`](super::Consumer) filled in for lost packets, as
/// [`Consumer::losses`](super::Consumer::losses) counts them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Losses {
	/// Lost packets rebuilt from redundancy a later packet carried.
	pub recovered: u64,
	/// Lost packets the codec concealed: no later packet carried them.
	pub concealed: u64,
	/// Holes too long to bridge, reported as [`Read::Gap`](super::Read::Gap) instead.
	pub gaps: u64,
}
