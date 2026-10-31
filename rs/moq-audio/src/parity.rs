//! Transport-layer parity over a track's encoded packets.
//!
//! One parity block covers a window of consecutive packets: the XOR of their
//! payloads plus the member stamps, carried as its own frame on a sidecar track
//! (`{track}.parity`). Codec-independent — parity covers the encoded bytes, so
//! it recovers what in-band FEC and deep redundancy cannot: packets of codecs
//! without their own recovery, and holes longer than the codec's cover.
//!
//! A receiver that misses one packet of a block reconstructs it as the XOR of
//! the parity and the block's other members. A block with two or more missing
//! members recovers nothing: that is the "one parity block" a block code
//! returns, and why the writer's window bounds both the wire cost (1/window of
//! the payload) and how long a receiver must wait for the block to close.

use std::collections::BTreeMap;

use bytes::{BufMut, Bytes, BytesMut};

/// Sidecar track name for a media track.
pub fn sidecar_name(track: &str) -> String {
	format!("{track}.parity")
}

/// Largest window a block can carry.
pub const MAX_WINDOW: usize = 16;

/// Stamp matching tolerance in microseconds.
///
/// Member stamps are the producer's packet stamps, but a caller reconstructing
/// a hole measures its span with clock math (`advance`/`rewind`) that can land
/// a microsecond off. A millisecond is far below the shortest packet (2.5 ms)
/// and far above any rounding, so members match by proximity without ever
/// confusing neighbouring packets.
const MATCH_TOLERANCE_US: u64 = 1_000;

const MAGIC: &[u8; 4] = b"PAR1";
const VERSION: u8 = 1;
/// magic + version + count + reserved.
const HEADER: usize = 8;
/// Stamp and length of one member.
const MEMBER: usize = 12;

/// Accumulates packets into XOR parity blocks.
pub struct Writer {
	window: usize,
	members: Vec<(u64, u32)>,
	xor: Vec<u8>,
}

impl Writer {
	/// A writer covering `window` packets per block. Below 2 or above
	/// [`MAX_WINDOW`] clamps into range.
	pub fn new(window: usize) -> Self {
		Self {
			window: window.clamp(2, MAX_WINDOW),
			members: Vec::new(),
			xor: Vec::new(),
		}
	}

	/// Feed the next packet in publication order. Returns the encoded block
	/// when this packet closes one, else `None`.
	pub fn push(&mut self, stamp_us: u64, payload: &[u8]) -> Option<Bytes> {
		// Media packets are far below 4 GiB; parity over a jumbo one is not a case.
		let len = u32::try_from(payload.len()).ok()?;
		if self.xor.len() < payload.len() {
			self.xor.resize(payload.len(), 0);
		}
		for (x, b) in self.xor.iter_mut().zip(payload) {
			*x ^= b;
		}
		self.members.push((stamp_us, len));
		(self.members.len() >= self.window).then(|| self.take())
	}

	/// Drop the partial block, e.g. at a timeline discontinuity: its members
	/// straddle codec resets the receiver shows as holes, which parity must
	/// not paper over.
	pub fn reset(&mut self) {
		self.members.clear();
		self.xor.clear();
	}

	fn take(&mut self) -> Bytes {
		let mut out = BytesMut::with_capacity(HEADER + MEMBER * self.members.len() + self.xor.len());
		out.extend_from_slice(MAGIC);
		out.put_u8(VERSION);
		out.put_u8(self.members.len() as u8);
		out.put_u16(0);
		for (stamp_us, len) in &self.members {
			out.put_u64_le(*stamp_us);
			out.put_u32_le(*len);
		}
		out.extend_from_slice(&self.xor);
		self.reset();
		out.freeze()
	}
}

/// One encoded parity block.
struct Block {
	/// Packet stamps and payload lengths, in publication order.
	members: Vec<(u64, u32)>,
	/// XOR of the members' payloads, each zero-padded to the longest.
	xor: Vec<u8>,
}

fn decode_block(blob: &[u8]) -> Option<Block> {
	if blob.len() < HEADER || &blob[0..4] != MAGIC || blob[4] != VERSION {
		return None;
	}
	let count = blob[5] as usize;
	if !(2..=MAX_WINDOW).contains(&count) {
		return None;
	}
	let mut members = Vec::with_capacity(count);
	let mut offset = HEADER;
	for _ in 0..count {
		let entry = blob.get(offset..offset + MEMBER)?;
		let stamp_us = u64::from_le_bytes(entry[0..8].try_into().ok()?);
		let len = u32::from_le_bytes(entry[8..12].try_into().ok()?);
		members.push((stamp_us, len));
		offset += MEMBER;
	}
	let xor = blob.get(offset..)?.to_vec();
	let max = members.iter().map(|(_, len)| *len as usize).max().unwrap_or(0);
	(xor.len() == max).then_some(Block { members, xor })
}

/// Reconstructs missing packets from offered blocks.
#[derive(Default)]
pub struct Reader {
	/// Packets seen (recorded arrivals plus reconstructions), by stamp.
	known: BTreeMap<u64, Bytes>,
	/// Blocks offered and not yet spent.
	blocks: Vec<Block>,
}

impl Reader {
	pub fn new() -> Self {
		Self::default()
	}

	/// Record an arriving packet.
	pub fn record(&mut self, stamp_us: u64, payload: Bytes) {
		self.known.insert(stamp_us, payload);
		self.prune(stamp_us);
	}

	/// Offer an encoded parity block (as read from the sidecar track).
	pub fn offer(&mut self, blob: &[u8]) {
		if let Some(block) = decode_block(blob) {
			self.blocks.push(block);
			// A block covers `window` recent packets; a handful is all a hole can use.
			let excess = self.blocks.len().saturating_sub(8);
			self.blocks.drain(..excess);
		}
	}

	/// Forget everything, e.g. at a timeline discontinuity.
	pub fn reset(&mut self) {
		self.known.clear();
		self.blocks.clear();
	}

	/// Reconstruct the missing packets of the hole `from_us..to_us` that a
	/// complete block can cover, in stamp order. Reconstructions count as known
	/// afterwards, so repeated calls are stable and overlapping blocks chain.
	pub fn reconstruct_span(&mut self, from_us: u64, to_us: u64) -> Vec<(u64, Bytes)> {
		let in_span = |stamp_us: u64| {
			stamp_us + MATCH_TOLERANCE_US >= from_us && stamp_us < to_us.saturating_add(MATCH_TOLERANCE_US)
		};

		let mut out = Vec::new();
		for block in &self.blocks {
			for (stamp_us, len) in &block.members {
				let (stamp_us, len) = (*stamp_us, *len);
				if in_span(stamp_us) && !self.known.contains_key(&stamp_us) {
					if let Some(payload) = reconstruct(block, &self.known, stamp_us, len) {
						out.push((stamp_us, payload));
					}
				}
			}
		}
		for (stamp_us, payload) in &out {
			self.known.insert(*stamp_us, payload.clone());
		}
		out.sort_by_key(|(stamp_us, _)| *stamp_us);
		out
	}

	/// Keep known packets around the newest stamp only: a hole's block reaches
	/// back one window, so a few windows of history is all that can matter.
	fn prune(&mut self, newest_us: u64) {
		let floor = newest_us.saturating_sub(5_000_000);
		while let Some((&oldest, _)) = self.known.iter().next() {
			if oldest < floor {
				self.known.remove(&oldest);
			} else {
				break;
			}
		}
	}
}

/// XOR the target packet out of a block whose every other member is known.
fn reconstruct(block: &Block, known: &BTreeMap<u64, Bytes>, stamp_us: u64, len: u32) -> Option<Bytes> {
	let mut payload = block.xor.clone();
	for (member_us, member_len) in &block.members {
		if *member_us == stamp_us {
			continue;
		}
		let member = known.get(member_us)?;
		debug_assert_eq!(member.len() as u32, *member_len);
		for (x, b) in payload.iter_mut().zip(member.iter()) {
			*x ^= b;
		}
	}
	payload.truncate(len as usize);
	Some(payload.into())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn writer(window: usize) -> Writer {
		Writer::new(window)
	}

	#[test]
	fn roundtrips_a_block() {
		let mut w = writer(3);
		assert!(w.push(0, b"aaaa").is_none());
		assert!(w.push(20_000, b"bbbbb").is_none());
		let blob = w.push(40_000, b"cc").expect("block closes");
		let block = decode_block(&blob).expect("decodes");
		assert_eq!(block.members, vec![(0, 4), (20_000, 5), (40_000, 2)]);
		assert_eq!(block.xor.len(), 5);
	}

	#[test]
	fn reconstructs_one_missing_packet() {
		let mut w = writer(3);
		w.push(0, b"aaaa");
		w.push(20_000, b"bbbbb");
		let blob = w.push(40_000, b"cc").unwrap();

		let mut r = Reader::new();
		r.record(0, Bytes::from_static(b"aaaa"));
		// 20_000 lost.
		r.record(40_000, Bytes::from_static(b"cc"));
		r.offer(&blob);

		let got = r.reconstruct_span(19_000, 21_000);
		assert_eq!(got, vec![(20_000, Bytes::from_static(b"bbbbb"))]);
	}

	#[test]
	fn two_missing_members_recover_nothing() {
		let mut w = writer(3);
		w.push(0, b"aaaa");
		w.push(20_000, b"bbbbb");
		let blob = w.push(40_000, b"cc").unwrap();

		let mut r = Reader::new();
		r.record(0, Bytes::from_static(b"aaaa"));
		r.offer(&blob);
		assert!(r.reconstruct_span(19_000, 41_000).is_empty());
	}

	#[test]
	fn reconstruction_counts_as_known_for_the_next_call() {
		let mut w = writer(3);
		w.push(0, b"aaaa");
		w.push(20_000, b"bbbbb");
		let blob = w.push(40_000, b"cc").unwrap();

		let mut r = Reader::new();
		r.record(0, Bytes::from_static(b"aaaa"));
		r.record(40_000, Bytes::from_static(b"cc"));
		r.offer(&blob);
		assert_eq!(r.reconstruct_span(19_000, 21_000).len(), 1);
		// Stable: the reconstruction is known now, not reconstructed twice.
		assert!(r.reconstruct_span(19_000, 21_000).is_empty());
	}

	#[test]
	fn span_matching_tolerates_a_clock_rounding() {
		let mut w = writer(2);
		w.push(20_000, b"aaaa");
		let blob = w.push(40_000, b"bb").unwrap();

		let mut r = Reader::new();
		r.record(40_000, Bytes::from_static(b"bb"));
		r.offer(&blob);
		// The caller's span math lands a microsecond under the member stamp.
		let got = r.reconstruct_span(19_999, 39_999);
		assert_eq!(got, vec![(20_000, Bytes::from_static(b"aaaa"))]);
	}

	#[test]
	fn ignores_foreign_blobs() {
		let mut r = Reader::new();
		r.offer(b"not a parity block");
		assert!(r.reconstruct_span(0, 1_000_000).is_empty());
	}

	#[test]
	fn empty_payloads_reconstruct_to_empty() {
		let mut w = writer(2);
		w.push(0, b"");
		let blob = w.push(20_000, b"aa").unwrap();
		let mut r = Reader::new();
		r.record(20_000, Bytes::from_static(b"aa"));
		r.offer(&blob);
		assert_eq!(r.reconstruct_span(0, 1), vec![(0, Bytes::new())]);
	}
}
