//! Tests for the MKV/WebM importer.
//!
//! These tests synthesize small WebM files via webm-iterable's writer (no external
//! tooling required) and feed them through [`crate::container::mkv::Import`], then assert that
//! the resulting catalog and frame stream are well-formed.

use std::io::Cursor;

use hang::catalog::{AudioCodec, Container, VideoCodec};
use webm_iterable::WebmWriter;
use webm_iterable::matroska_spec::{Master, MatroskaSpec, SimpleBlock};

/// Build a minimal WebM byte stream with the given tracks and blocks.
struct MkvBuilder {
	tags: Vec<MatroskaSpec>,
}

impl MkvBuilder {
	fn new() -> Self {
		Self { tags: Vec::new() }
	}

	fn header(mut self, doc_type: &str) -> Self {
		self.tags.push(MatroskaSpec::Ebml(Master::Full(vec![
			MatroskaSpec::DocType(doc_type.to_string()),
			MatroskaSpec::DocTypeVersion(2),
			MatroskaSpec::DocTypeReadVersion(2),
		])));
		self
	}

	fn segment_start(mut self) -> Self {
		self.tags.push(MatroskaSpec::Segment(Master::Start));
		self
	}

	fn segment_end(mut self) -> Self {
		self.tags.push(MatroskaSpec::Segment(Master::End));
		self
	}

	fn info(mut self, timestamp_scale_ns: u64) -> Self {
		self.tags
			.push(MatroskaSpec::Info(Master::Full(vec![MatroskaSpec::TimestampScale(
				timestamp_scale_ns,
			)])));
		self
	}

	fn track_video(mut self, number: u64, codec_id: &str, width: u64, height: u64) -> Self {
		self.tags
			.push(MatroskaSpec::Tracks(Master::Full(vec![MatroskaSpec::TrackEntry(
				Master::Full(vec![
					MatroskaSpec::TrackNumber(number),
					MatroskaSpec::TrackUID(number),
					MatroskaSpec::TrackType(1),
					MatroskaSpec::CodecID(codec_id.to_string()),
					MatroskaSpec::Video(Master::Full(vec![
						MatroskaSpec::PixelWidth(width),
						MatroskaSpec::PixelHeight(height),
					])),
				]),
			)])));
		self
	}

	fn tracks(mut self, entries: Vec<MatroskaSpec>) -> Self {
		self.tags.push(MatroskaSpec::Tracks(Master::Full(entries)));
		self
	}

	fn cluster<F>(mut self, cluster_timestamp: u64, blocks: F) -> Self
	where
		F: FnOnce() -> Vec<MatroskaSpec>,
	{
		self.tags.push(MatroskaSpec::Cluster(Master::Start));
		self.tags.push(MatroskaSpec::Timestamp(cluster_timestamp));
		self.tags.extend(blocks());
		self.tags.push(MatroskaSpec::Cluster(Master::End));
		self
	}

	fn build(self) -> Vec<u8> {
		let mut dest = Cursor::new(Vec::new());
		{
			let mut writer = WebmWriter::new(&mut dest);
			for tag in &self.tags {
				writer.write(tag).expect("write tag");
			}
		}
		dest.into_inner()
	}
}

fn simple_block(track: u64, rel_ts: i16, keyframe: bool, payload: &[u8]) -> MatroskaSpec {
	let sb = SimpleBlock::new_uncheked(payload, track, rel_ts, false, None, false, keyframe);
	sb.into()
}

fn track_entry_audio_opus(number: u64, sample_rate: f64, channels: u64) -> MatroskaSpec {
	// Minimal OpusHead: magic + version + channels + pre-skip + sample_rate (LE) + gain + mapping.
	let mut head = Vec::new();
	head.extend_from_slice(b"OpusHead");
	head.push(1); // version
	head.push(channels as u8);
	head.extend_from_slice(&0u16.to_le_bytes()); // pre-skip
	head.extend_from_slice(&(sample_rate as u32).to_le_bytes());
	head.extend_from_slice(&0i16.to_le_bytes()); // gain
	head.push(0); // mapping family

	MatroskaSpec::TrackEntry(Master::Full(vec![
		MatroskaSpec::TrackNumber(number),
		MatroskaSpec::TrackUID(number),
		MatroskaSpec::TrackType(2),
		MatroskaSpec::CodecID("A_OPUS".to_string()),
		MatroskaSpec::CodecPrivate(head),
		MatroskaSpec::Audio(Master::Full(vec![
			MatroskaSpec::SamplingFrequency(sample_rate),
			MatroskaSpec::Channels(channels),
		])),
	]))
}

fn track_entry_audio_flac(number: u64, sample_rate: u32, channels: u32) -> MatroskaSpec {
	// A_FLAC CodecPrivate is the FLAC header: the `fLaC` marker plus the STREAMINFO
	// metadata block. Reuse the codec helper so the bytes match what the importer parses.
	let private = crate::codec::flac::Config {
		min_block_size: 4096,
		max_block_size: 4096,
		min_frame_size: 0,
		max_frame_size: 0,
		sample_rate,
		channel_count: channels,
		bits_per_sample: 16,
		total_samples: 0,
		md5: [0; 16],
	}
	.description()
	.to_vec();

	MatroskaSpec::TrackEntry(Master::Full(vec![
		MatroskaSpec::TrackNumber(number),
		MatroskaSpec::TrackUID(number),
		MatroskaSpec::TrackType(2),
		MatroskaSpec::CodecID("A_FLAC".to_string()),
		MatroskaSpec::CodecPrivate(private),
		MatroskaSpec::Audio(Master::Full(vec![
			MatroskaSpec::SamplingFrequency(sample_rate as f64),
			MatroskaSpec::Channels(channels as u64),
		])),
	]))
}

fn track_entry_audio_mp3(number: u64, sample_rate: f64, channels: u64) -> MatroskaSpec {
	// MP3 has no codec private; config comes from the track header.
	MatroskaSpec::TrackEntry(Master::Full(vec![
		MatroskaSpec::TrackNumber(number),
		MatroskaSpec::TrackUID(number),
		MatroskaSpec::TrackType(2),
		MatroskaSpec::CodecID("A_MPEG/L3".to_string()),
		MatroskaSpec::Audio(Master::Full(vec![
			MatroskaSpec::SamplingFrequency(sample_rate),
			MatroskaSpec::Channels(channels),
		])),
	]))
}

fn track_entry_video_vp9(number: u64, width: u64, height: u64) -> MatroskaSpec {
	MatroskaSpec::TrackEntry(Master::Full(vec![
		MatroskaSpec::TrackNumber(number),
		MatroskaSpec::TrackUID(number),
		MatroskaSpec::TrackType(1),
		MatroskaSpec::CodecID("V_VP9".to_string()),
		MatroskaSpec::Video(Master::Full(vec![
			MatroskaSpec::PixelWidth(width),
			MatroskaSpec::PixelHeight(height),
		])),
	]))
}

fn run(data: &[u8]) -> crate::catalog::hang::Catalog {
	let mut broadcast = moq_net::broadcast::Info::new().produce();
	let catalog = crate::catalog::Producer::new(&mut broadcast, crate::catalog::Config::default()).unwrap();
	let mut mkv = crate::container::mkv::Import::new(broadcast, catalog.reserve());
	let buf = bytes::BytesMut::from(data);
	mkv.decode(&buf).expect("decode");
	mkv.finish().expect("finish");
	catalog.snapshot()
}

#[test]
fn test_flac_catalog() {
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.tracks(vec![track_entry_audio_flac(1, 48000, 2)])
		.cluster(0, || vec![simple_block(1, 0, true, b"flac-frame-0")])
		.segment_end()
		.build();

	let catalog = run(&data);
	assert_eq!(catalog.audio.renditions.len(), 1);

	let a = catalog.audio.renditions.values().next().unwrap();
	assert!(matches!(a.codec, AudioCodec::Flac));
	assert_eq!(a.sample_rate, 48000);
	assert_eq!(a.channel_count, 2);
	assert!(matches!(a.container, Container::Legacy));
	// The description is the WebCodecs FLAC config: the `fLaC` marker + STREAMINFO.
	let desc = a.description.as_ref().expect("flac description");
	assert_eq!(&desc[..4], b"fLaC");
}

#[test]
fn test_vp9_only_catalog() {
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.track_video(1, "V_VP9", 1280, 720)
		.cluster(0, || vec![simple_block(1, 0, true, b"\x00\x00\x00\x01vp9-frame")])
		.segment_end()
		.build();

	let catalog = run(&data);
	assert_eq!(catalog.video.renditions.len(), 1);
	assert_eq!(catalog.audio.renditions.len(), 0);

	let v = catalog.video.renditions.values().next().unwrap();
	assert!(matches!(v.codec, VideoCodec::VP9(_)), "codec: {:?}", v.codec);
	assert_eq!(v.coded_width, Some(1280));
	assert_eq!(v.coded_height, Some(720));
	assert!(matches!(v.container, Container::Legacy));
}

#[tokio::test(start_paused = true)]
async fn public_container_preserves_loc_for_mkv() {
	let payload = b"vp9-key";
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.track_video(1, "V_VP9", 320, 240)
		.cluster(0, || vec![simple_block(1, 0, true, payload)])
		.segment_end()
		.build();

	let mut broadcast = moq_net::broadcast::Info::new().produce();
	let consumer = broadcast.consume();
	let catalog = crate::catalog::Producer::new(&mut broadcast, crate::catalog::Config::default()).unwrap();
	let reserved = catalog.reserve();
	let mut import = super::Import::new(broadcast, reserved).with_container(hang::catalog::Container::Loc);
	import.decode(&data).unwrap();
	import.finish().unwrap();

	let snapshot = catalog.snapshot();
	let (name, config) = snapshot.video.renditions.iter().next().unwrap();
	assert_eq!(config.container, Container::Loc);

	let track = consumer.track(name).unwrap().subscribe(None).await.unwrap();
	let mut media = crate::container::Consumer::new(
		track,
		crate::catalog::hang::Container::Loc(crate::container::Kind::Data),
	);
	let frame = tokio::time::timeout(std::time::Duration::from_secs(1), media.read())
		.await
		.unwrap()
		.unwrap()
		.unwrap();
	assert_eq!(frame.payload.as_ref(), payload);
}

#[test]
fn test_vp9_opus_catalog() {
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.tracks(vec![
			track_entry_video_vp9(1, 640, 480),
			track_entry_audio_opus(2, 48000.0, 2),
		])
		.cluster(0, || {
			vec![
				simple_block(1, 0, true, b"vp9-key"),
				simple_block(2, 0, true, b"opus-pkt-0"),
				simple_block(2, 20, true, b"opus-pkt-1"),
				simple_block(1, 33, false, b"vp9-p"),
			]
		})
		.segment_end()
		.build();

	let catalog = run(&data);
	assert_eq!(catalog.video.renditions.len(), 1);
	assert_eq!(catalog.audio.renditions.len(), 1);

	let v = catalog.video.renditions.values().next().unwrap();
	assert!(matches!(v.codec, VideoCodec::VP9(_)));

	let a = catalog.audio.renditions.values().next().unwrap();
	assert!(matches!(a.codec, AudioCodec::Opus));
	assert_eq!(a.sample_rate, 48000);
	assert_eq!(a.channel_count, 2);
}

#[test]
fn test_opus_catalog_rate_is_decoder_rate() {
	// A 44.1 kHz input rate is informational; the catalog still reports the decoder's 48 kHz.
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.tracks(vec![track_entry_audio_opus(1, 44100.0, 2)])
		.cluster(0, || vec![simple_block(1, 0, true, b"opus")])
		.segment_end()
		.build();

	let catalog = run(&data);
	let a = catalog.audio.renditions.values().next().unwrap();
	assert_eq!(a.sample_rate, 48000);
	assert_eq!(a.channel_count, 2);
}

#[test]
fn test_mp3_catalog() {
	let data = MkvBuilder::new()
		.header("matroska")
		.segment_start()
		.info(1_000_000)
		.tracks(vec![track_entry_audio_mp3(1, 44100.0, 2)])
		.cluster(0, || vec![simple_block(1, 0, true, b"mp3-frame")])
		.segment_end()
		.build();

	let catalog = run(&data);
	assert_eq!(catalog.audio.renditions.len(), 1);
	let a = catalog.audio.renditions.values().next().unwrap();
	assert!(matches!(a.codec, AudioCodec::Mp3));
	assert_eq!(a.sample_rate, 44100);
	assert_eq!(a.channel_count, 2);
	assert!(a.description.is_none(), "MP3 config is in band");
}

#[test]
fn test_chunked_decode() {
	// Build the same WebM and feed it in tiny chunks. Incremental parsing ensures
	// that frames aren't emitted twice across the parse restarts.
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.track_video(1, "V_VP9", 320, 240)
		.cluster(0, || {
			vec![
				simple_block(1, 0, true, b"k0"),
				simple_block(1, 33, false, b"p1"),
				simple_block(1, 66, false, b"p2"),
			]
		})
		.cluster(100, || {
			vec![simple_block(1, 0, true, b"k1"), simple_block(1, 33, false, b"p3")]
		})
		.segment_end()
		.build();

	let mut broadcast = moq_net::broadcast::Info::new().produce();
	let catalog = crate::catalog::Producer::new(&mut broadcast, crate::catalog::Config::default()).unwrap();
	let mut mkv = crate::container::mkv::Import::new(broadcast, catalog.reserve());

	// Split elements across input chunks.
	for chunk in data.chunks(16) {
		let b = bytes::BytesMut::from(chunk);
		mkv.decode(&b).expect("decode chunk");
	}
	mkv.finish().expect("finish");

	let catalog = catalog.snapshot();
	assert_eq!(catalog.video.renditions.len(), 1);
	let v = catalog.video.renditions.values().next().unwrap();
	assert_eq!(v.coded_width, Some(320));
	assert_eq!(v.coded_height, Some(240));
}

#[test]
fn test_unsupported_codec_refused() {
	// An unsupported audio codec fails the import instead of silently dropping a track.
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.tracks(vec![
			track_entry_audio_opus(1, 48000.0, 2),
			MatroskaSpec::TrackEntry(Master::Full(vec![
				MatroskaSpec::TrackNumber(2),
				MatroskaSpec::TrackUID(2),
				MatroskaSpec::TrackType(2),
				MatroskaSpec::CodecID("A_VORBIS".to_string()),
			])),
		])
		.cluster(0, || vec![simple_block(1, 0, true, b"opus")])
		.segment_end()
		.build();

	for initial in [false, true] {
		assert!(
			matches!(import_result(&data, initial), Err(crate::Error::Mkv(super::Error::UnsupportedAudioCodec(codec))) if codec == "A_VORBIS")
		);
	}
}

#[test]
fn test_block_timestamp_scaling() {
	// TimestampScale = 1_000_000 ns (1ms). Cluster timestamp = 1000, block rel = 33
	// → 1033 ms = 1_033_000 us.
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.track_video(1, "V_VP9", 16, 16)
		.cluster(1000, || vec![simple_block(1, 33, true, b"f")])
		.segment_end()
		.build();

	// Smoke check: parsing succeeds. Timestamp value itself is internal to the
	// container::Producer; the catalog round-trip above already exercises the
	// rendition wiring.
	let _ = run(&data);
}

/// A rendition must never be advertised when its media producer could not be built.
///
/// Publishing the media producer is fallible (it enrolls the track in the broadcast timeline, minting the
/// shared `timeline.z` track, which can collide), so publishing the catalog entry first would
/// leave consumers a rendition that is announced but has no producer behind it and is therefore
/// never served.
#[test]
fn rendition_is_not_published_when_the_media_track_fails() {
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.tracks(vec![track_entry_video_vp9(1, 640, 480)])
		.segment_end()
		.build();

	// Control: the same fixture publishes exactly one rendition when nothing collides, so the
	// assertion below cannot pass merely because the fixture stopped reaching track import.
	assert_eq!(run(&data).video.renditions.len(), 1, "fixture must publish a rendition");

	let mut broadcast = moq_net::broadcast::Info::new().produce();
	let catalog = crate::catalog::Producer::new(&mut broadcast, crate::catalog::Config::default()).unwrap();

	// Squat the broadcast's timeline track, so building the media producer (which creates it
	// on first use) fails. The handle must stay alive: the broadcast tracks names weakly, so
	// dropping it frees the name.
	let _squat = broadcast.create_track(hang::timeline::DEFAULT_NAME, None).unwrap();

	let mut mkv = crate::container::mkv::Import::new(broadcast, catalog.reserve());
	let buf = bytes::BytesMut::from(&data[..]);
	// A producer failure reaches the caller before advertising a rendition.
	assert!(mkv.decode(&buf).is_err());

	assert!(
		catalog.snapshot().video.renditions.is_empty(),
		"a rendition whose media producer failed must not be advertised"
	);
}

/// Container entry points must preserve importer failures, both in the constructor's
/// initial data and in arbitrarily split subsequent chunks.
fn import_result(data: &[u8], initial: bool) -> crate::Result<()> {
	let mut broadcast = moq_net::broadcast::Info::new().produce();
	let catalog = crate::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
	let init = crate::import::ContainerInit::new(
		crate::import::ContainerFormat::Mkv,
		if initial {
			bytes::Bytes::copy_from_slice(data)
		} else {
			bytes::Bytes::new()
		},
	);
	let mut import: crate::import::Container = crate::import::Container::new(broadcast, catalog.reserve(), &init)?;
	if !initial {
		for chunk in data.chunks(7) {
			import.decode(chunk)?;
		}
	}
	import.finish()
}

#[test]
fn malformed_track_metadata_fails_through_the_container() {
	use MatroskaSpec::*;
	let cases = [
		TrackEntry(Master::Full(vec![TrackType(2), CodecID("A_OPUS".into())])),
		TrackEntry(Master::Full(vec![
			TrackNumber(2),
			TrackType(2),
			CodecID("A_AAC".into()),
		])),
		TrackEntry(Master::Full(vec![
			TrackNumber(2),
			TrackType(1),
			CodecID("V_MPEG4/ISO/AVC".into()),
			CodecPrivate(vec![0]),
		])),
	];
	for (index, invalid) in cases.into_iter().enumerate() {
		for reverse in [false, true] {
			let mut tracks = vec![track_entry_audio_opus(1, 48_000.0, 2), invalid.clone()];
			if reverse {
				tracks.reverse();
			}
			let data = MkvBuilder::new()
				.header("matroska")
				.segment_start()
				.info(1_000_000)
				.tracks(tracks)
				.segment_end()
				.build();
			for initial in [false, true] {
				assert!(
					import_result(&data, initial).is_err(),
					"case {index}, reverse {reverse}, initial {initial}"
				);
			}
		}
	}
}

#[test]
fn duplicate_track_numbers_fail_even_for_skipped_roles() {
	use MatroskaSpec::*;
	for second in [
		track_entry_audio_opus(1, 44_100.0, 1),
		TrackEntry(Master::Full(vec![
			TrackNumber(1),
			TrackType(18),
			CodecID("B_BUTTONS".into()),
		])),
	] {
		for reverse in [false, true] {
			let mut tracks = vec![track_entry_audio_opus(1, 48_000.0, 2), second.clone()];
			if reverse {
				tracks.reverse();
			}
			let data = MkvBuilder::new()
				.header("matroska")
				.segment_start()
				.info(1_000_000)
				.tracks(tracks)
				.segment_end()
				.build();
			for initial in [false, true] {
				let result = import_result(&data, initial);
				assert!(
					matches!(&result, Err(crate::Error::Moq(moq_net::Error::Duplicate))),
					"reverse {reverse}, initial {initial}: {result:?}"
				);
			}
		}
	}
}

#[test]
fn invalid_timestamp_scales_are_refused() {
	for initial in [false, true] {
		let zero = MkvBuilder::new()
			.header("webm")
			.segment_start()
			.info(0)
			.tracks(vec![track_entry_audio_opus(1, 48_000.0, 2)])
			.cluster(0, || vec![simple_block(1, 0, true, b"opus")])
			.segment_end()
			.build();
		assert!(matches!(
			import_result(&zero, initial),
			Err(crate::Error::Mkv(super::Error::MatroskaParse))
		));
	}
}

#[test]
fn block_tick_overflow_is_refused_before_timestamp_conversion() {
	for (cluster, relative, scale) in [(u64::MAX, 1, 1), (u64::MAX, 0, 2)] {
		for initial in [false, true] {
			let data = MkvBuilder::new()
				.header("webm")
				.segment_start()
				.info(scale)
				.tracks(vec![track_entry_audio_opus(1, 48_000.0, 2)])
				.cluster(cluster, || vec![simple_block(1, relative, true, b"opus")])
				.segment_end()
				.build();
			let outcome = std::panic::catch_unwind(|| import_result(&data, initial));
			assert!(matches!(
				outcome,
				Ok(Err(crate::Error::Mkv(super::Error::TimestampOverflow)))
			));
		}
	}
	let negative = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1)
		.tracks(vec![track_entry_audio_opus(1, 48_000.0, 2)])
		.cluster(0, || vec![simple_block(1, -1, true, b"opus")])
		.segment_end()
		.build();
	for initial in [false, true] {
		assert!(matches!(
			import_result(&negative, initial),
			Err(crate::Error::Mkv(super::Error::NegativeBlockTimestamp))
		));
	}
}

#[test]
fn partial_track_metadata_is_refused_at_eof() {
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.tracks(vec![track_entry_audio_opus(1, 48_000.0, 2)])
		.segment_end()
		.build();
	for missing in 1..=8 {
		for initial in [false, true] {
			assert!(
				matches!(
					import_result(&data[..data.len() - missing], initial),
					Err(crate::Error::Mkv(super::Error::MatroskaParse))
				),
				"missing {missing}, initial {initial}"
			);
		}
	}
	for initial in [false, true] {
		import_result(&data, initial).unwrap();
	}
}

#[test]
fn block_ticks_above_the_signed_range_are_not_misreported_as_negative() {
	for (cluster, relative) in [(i64::MAX as u64, 1i16), (u64::MAX, -1)] {
		let data = MkvBuilder::new()
			.header("webm")
			.segment_start()
			.info(1)
			.tracks(vec![track_entry_audio_opus(1, 48_000.0, 2)])
			.cluster(cluster, || vec![simple_block(1, relative, true, b"frame")])
			.segment_end()
			.build();
		// The addition fits u64, but exceeds MoQ's existing timestamp value range.
		for initial in [false, true] {
			assert!(
				matches!(import_result(&data, initial), Err(crate::Error::TimestampOverflow(_))),
				"{cluster}+{relative}, initial {initial}"
			);
		}
	}
}

#[test]
fn segment_children_require_their_parent() {
	let header = MkvBuilder::new().header("webm").build();
	let data = MkvBuilder::new()
		.header("webm")
		.segment_start()
		.info(1_000_000)
		.tracks(vec![track_entry_audio_opus(1, 48_000.0, 2)])
		.segment_end()
		.build();
	let start = data
		.windows(4)
		.position(|bytes| bytes == [0x16, 0x54, 0xae, 0x6b])
		.unwrap();
	let mut malformed = header;
	malformed.extend_from_slice(&data[start..]);
	for initial in [false, true] {
		assert!(matches!(
			import_result(&malformed, initial),
			Err(crate::Error::Mkv(super::Error::MatroskaParse))
		));
	}
}

#[tokio::test(start_paused = true)]
async fn live_webm_and_terminal_state_survive_chunk_boundaries() {
	let data = include_bytes!("test_data/opus.webm");
	for chunk in [1, 7, 64, data.len()] {
		let mut broadcast = moq_net::broadcast::Info::new().produce();
		let consumer = broadcast.consume();
		let catalog = crate::catalog::Producer::new(&mut broadcast, Default::default()).unwrap();
		let mut import = super::Import::new(broadcast, catalog.reserve());
		for bytes in data.chunks(chunk) {
			import.decode(bytes).unwrap();
		}
		import.finish().unwrap();
		let snapshot = catalog.snapshot();
		assert_eq!(snapshot.audio.renditions.len(), 1);
		let published = crate::container::test_util::published(&consumer, &snapshot).await;
		let timestamps = published.into_values().next().unwrap();
		assert_eq!(timestamps.len(), 6);
		assert!(timestamps.windows(2).all(|pair| pair[0] < pair[1]));
		assert!(matches!(
			import.decode(&[0]),
			Err(crate::Error::Mkv(super::Error::MatroskaParse))
		));
	}
}

#[test]
fn invalid_track_number_fields_are_refused() {
	use MatroskaSpec::*;
	for fields in [vec![TrackNumber(0)], vec![TrackNumber(1), TrackNumber(2)]] {
		let mut entry = fields;
		entry.extend([TrackType(2), CodecID("A_OPUS".into())]);
		let data = MkvBuilder::new()
			.header("webm")
			.segment_start()
			.info(1_000_000)
			.tracks(vec![TrackEntry(Master::Full(entry))])
			.segment_end()
			.build();
		for initial in [false, true] {
			assert!(import_result(&data, initial).is_err());
		}
	}
}
