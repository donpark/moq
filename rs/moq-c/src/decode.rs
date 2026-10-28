//! The tagged reads encoded and decoded media consumers deliver.
//!
//! Encoded media and both decoders hand over the same three kinds of read, so a C player
//! handles loss and encoder ends alike for every media type.

use crate::{Error, moq_timestamp};

/// Which kind of media read was delivered: a frame, or a break between frames.
#[repr(C)]
#[expect(non_camel_case_types, reason = "C ABI type names match moq.h")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum moq_decode_read_kind {
	/// A media frame.
	MOQ_DECODE_READ_FRAME = 0,
	/// Media after what was already read was lost: a group was skipped or never arrived.
	/// What follows does not continue what came before.
	MOQ_DECODE_READ_GAP = 1,
	/// The publisher stopped its encoder: it paused, switched source, or finished. What
	/// follows comes from a fresh encoder.
	MOQ_DECODE_READ_END = 2,
}

/// Lost media, for a [moq_decode_read_kind::MOQ_DECODE_READ_GAP] read.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct moq_decode_gap {
	/// The furthest presentation point already read.
	pub timestamp: moq_timestamp,
	/// An upper bound on how long until the next frame resumes, in microseconds.
	pub duration_us: u64,
}

/// A delivered read held by its id.
#[derive(Clone)]
pub(crate) enum Read<F> {
	Frame(F),
	Gap(moq_decode_gap),
	End(moq_timestamp),
}

impl<F> Read<F> {
	pub(crate) fn gap(timestamp: moq_net::Timestamp, duration: moq_net::Timestamp) -> Result<Self, Error> {
		Ok(Self::Gap(moq_decode_gap {
			timestamp: timestamp.into(),
			duration_us: duration
				.as_micros()
				.try_into()
				.map_err(|_| Error::TimestampOverflow(moq_net::TimeOverflow))?,
		}))
	}

	pub(crate) fn end(end: moq_net::Timestamp) -> Self {
		Self::End(end.into())
	}

	/// The C kind, plus the gap and end fields a non-frame read fills.
	pub(crate) fn info(&self) -> (moq_decode_read_kind, moq_decode_gap, moq_timestamp) {
		match self {
			Self::Frame(_) => (
				moq_decode_read_kind::MOQ_DECODE_READ_FRAME,
				moq_decode_gap::default(),
				moq_timestamp::default(),
			),
			Self::Gap(gap) => (
				moq_decode_read_kind::MOQ_DECODE_READ_GAP,
				*gap,
				moq_timestamp::default(),
			),
			Self::End(end) => (
				moq_decode_read_kind::MOQ_DECODE_READ_END,
				moq_decode_gap::default(),
				*end,
			),
		}
	}
}
