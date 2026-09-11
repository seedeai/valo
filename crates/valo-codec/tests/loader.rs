mod support;

use pollster::block_on;
use std::time::Duration;
use support::*;
use valo_codec::{DecodeError, DecodeLimits, DecodeOptions, ImageLoader, Repetition};

#[test]
fn the_first_decoder_that_accepts_owns_the_image_and_later_ones_are_not_asked() {
    let log = Log::default();
    let loader = ImageLoader::new(
        images(),
        decoders([
            FakeDecoder::new("native").declining("animation"),
            FakeDecoder::new("software").sharing_log(&log),
            FakeDecoder::new("never").sharing_log(&log),
        ]),
    );
    let image = block_on(loader.decode(bytes(), DecodeOptions::default())).unwrap();
    assert_eq!(image.size(), [1, 1]);
    assert_eq!(
        log.events(),
        ["open:software", "frame:software:0", "drop:software"]
    );
}

#[test]
fn every_decoder_declining_reports_each_reason_and_a_failure_stops_the_search() {
    let loader = ImageLoader::new(
        images(),
        decoders([
            FakeDecoder::new("native").declining("no such container"),
            FakeDecoder::new("software").declining("unknown format"),
        ]),
    );
    match block_on(loader.decode(bytes(), DecodeOptions::default())) {
        Err(DecodeError::Unsupported(declined)) => {
            let names: Vec<_> = declined.iter().map(|d| d.decoder).collect();
            assert_eq!(names, ["native", "software"]);
            assert_eq!(declined[1].reason, "unknown format");
        }
        other => panic!("{other:?}"),
    }

    let log = Log::default();
    let loader = ImageLoader::new(
        images(),
        decoders([
            FakeDecoder::new("native").failing(DecodeError::InvalidData("truncated".into())),
            FakeDecoder::new("software").sharing_log(&log),
        ]),
    );
    assert!(matches!(
        block_on(loader.decode(bytes(), DecodeOptions::default())),
        Err(DecodeError::InvalidData(_))
    ));
    assert!(log.events().is_empty(), "a failure must not fall through");
}

#[test]
fn no_decoders_and_empty_or_oversized_input_are_rejected_before_any_decoder_runs() {
    assert!(matches!(
        block_on(ImageLoader::new(images(), Vec::new()).decode(bytes(), DecodeOptions::default())),
        Err(DecodeError::NoDecoder)
    ));
    let log = Log::default();
    let loader = ImageLoader::new(
        images(),
        decoders([FakeDecoder::new("software").sharing_log(&log)]),
    );
    assert!(matches!(
        block_on(loader.decode(std::sync::Arc::from([]), DecodeOptions::default())),
        Err(DecodeError::InvalidData(_))
    ));
    let tiny = DecodeOptions {
        limits: DecodeLimits {
            max_encoded_bytes: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(matches!(
        block_on(loader.decode(bytes(), tiny)),
        Err(DecodeError::LimitExceeded("encoded bytes"))
    ));
    assert!(log.events().is_empty());
}

#[test]
fn inline_work_happens_at_the_first_poll_and_never_if_dropped_first() {
    let log = Log::default();
    let loader = ImageLoader::new(
        images(),
        decoders([FakeDecoder::new("software").sharing_log(&log)]),
    );
    drop(loader.decode(bytes(), DecodeOptions::default()));
    assert!(log.events().is_empty());
    let mut pending = loader.decode(bytes(), DecodeOptions::default());
    assert!(log.events().is_empty());
    assert!(pending.try_take().unwrap().is_ok());
    assert_eq!(log.events().len(), 3);
}

#[test]
fn a_codec_hands_out_frames_in_order_and_wraps_after_the_last() {
    let loader = ImageLoader::new(
        images(),
        decoders([FakeDecoder::new("software").frames(3, Repetition::Times(2))]),
    );
    let mut codec = block_on(loader.open(bytes(), DecodeOptions::default())).unwrap();
    assert_eq!(codec.info().frame_count, 3);
    assert_eq!(codec.info().repetition, Repetition::Times(2));
    let durations: Vec<_> = (0..4)
        .map(|_| block_on(codec.next_frame()).unwrap().duration)
        .collect();
    assert_eq!(durations, [10, 11, 12, 10].map(Duration::from_millis));
}

#[test]
fn a_frame_failure_is_reported_and_the_codec_stays_usable() {
    let mut decoder = FakeDecoder::new("software").frames(2, Repetition::Forever);
    decoder.fail_frame = Some((1, DecodeError::InvalidData("bad chunk".into())));
    let loader = ImageLoader::new(images(), decoders([decoder]));
    let mut codec = block_on(loader.open(bytes(), DecodeOptions::default())).unwrap();
    assert!(block_on(codec.next_frame()).is_ok());
    assert!(matches!(
        block_on(codec.next_frame()),
        Err(DecodeError::InvalidData(_))
    ));
    assert!(block_on(codec.next_frame()).is_ok());
}

#[test]
fn max_size_is_passed_to_the_decoder_and_frames_come_back_at_that_size() {
    let mut decoder = FakeDecoder::new("software");
    decoder.size = [400, 200];
    let loader = ImageLoader::new(images(), decoders([decoder]));
    let options = DecodeOptions {
        max_size: Some([100, 100]),
        mipmaps: true,
        ..Default::default()
    };
    let mut codec = block_on(loader.open(bytes(), options)).unwrap();
    assert_eq!(codec.info().size, [100, 50]);
    let frame = block_on(codec.next_frame()).unwrap();
    assert_eq!(frame.image.size(), [100, 50]);
    assert_eq!(frame.image.mip_levels(), 7);
}

#[test]
fn limits_on_frames_and_pixels_apply_to_what_the_decoder_reports() {
    let loader = ImageLoader::new(
        images(),
        decoders([FakeDecoder::new("software").frames(5, Repetition::Forever)]),
    );
    let few_frames = DecodeOptions {
        limits: DecodeLimits {
            max_frames: 4,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(matches!(
        block_on(loader.open(bytes(), few_frames)),
        Err(DecodeError::LimitExceeded("animation frames"))
    ));
    let mut large = FakeDecoder::new("software");
    large.size = [1000, 1000];
    let loader = ImageLoader::new(images(), decoders([large]));
    let few_pixels = DecodeOptions {
        limits: DecodeLimits {
            max_pixels: 10,
            ..Default::default()
        },
        ..Default::default()
    };
    assert!(matches!(
        block_on(loader.decode(bytes(), few_pixels)),
        Err(DecodeError::LimitExceeded("decoded pixels"))
    ));
}

#[test]
fn a_decoder_that_answers_later_is_waited_for_rather_than_assumed_ready() {
    let (decoder, release) = DelayedDecoder::new("browser");
    let loader = ImageLoader::new(images(), vec![Box::new(decoder)]);
    let mut decoding = loader.decode(bytes(), DecodeOptions::default());
    assert!(
        decoding.try_take().is_none(),
        "nothing to take while the decoder is still working"
    );
    release.release();
    let image = decoding.try_take().expect("the answer arrived").unwrap();
    assert_eq!(image.size(), [1, 1]);
}
