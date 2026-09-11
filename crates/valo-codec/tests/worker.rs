#![cfg(all(feature = "worker", not(target_arch = "wasm32")))]
mod support;

use pollster::block_on;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;
use support::*;
use valo_codec::{DecodeError, DecodeOptions, ImageLoader, Repetition};

fn wait_for(log: &Log, count: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while log.events().len() < count {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out: {:?}",
            log.events()
        );
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn decoding_runs_on_the_worker_and_the_result_wakes_the_caller() {
    let log = Log::default();
    let loader = ImageLoader::with_worker(
        images(),
        decoders([FakeDecoder::new("software").sharing_log(&log)]),
    )
    .unwrap();
    // pollster parks on a condvar: completing at all proves the worker woke us.
    let image = block_on(loader.decode(bytes(), DecodeOptions::default())).unwrap();
    assert_eq!(image.size(), [1, 1]);
    let threads = log.threads();
    assert_eq!(threads.len(), 3);
    assert!(threads.iter().all(|t| *t != thread::current().id()));
    assert!(threads.iter().all(|t| *t == threads[0]));
}

#[test]
fn a_codec_keeps_its_reader_on_the_worker_and_releases_it_when_dropped() {
    let log = Log::default();
    let loader = ImageLoader::with_worker(
        images(),
        decoders([FakeDecoder::new("software")
            .frames(2, Repetition::Forever)
            .sharing_log(&log)]),
    )
    .unwrap();
    let mut codec = block_on(loader.open(bytes(), DecodeOptions::default())).unwrap();
    let first = block_on(codec.next_frame()).unwrap();
    let second = block_on(codec.next_frame()).unwrap();
    assert_eq!(first.duration, Duration::from_millis(10));
    assert_eq!(second.duration, Duration::from_millis(11));
    drop(loader);
    drop(codec);
    wait_for(&log, 4);
    assert_eq!(
        log.events(),
        [
            "open:software",
            "frame:software:0",
            "frame:software:1",
            "drop:software"
        ]
    );
}

#[test]
fn cancelling_an_open_in_flight_releases_the_late_reader_on_the_worker() {
    let log = Log::default();
    let (release, gate) = mpsc::channel();
    let mut decoder = FakeDecoder::new("software").sharing_log(&log);
    decoder.open_gate = Some(Arc::new(Mutex::new(gate)));
    let loader = ImageLoader::with_worker(images(), decoders([decoder])).unwrap();
    let mut pending = loader.open(bytes(), DecodeOptions::default());
    assert!(pending.try_take().is_none());
    wait_for(&log, 1);
    drop(pending);
    release.send(()).unwrap();
    wait_for(&log, 2);
    assert_eq!(log.events(), ["open:software", "drop:software"]);
    let threads = log.threads();
    assert_eq!(threads[0], threads[1]);
}

#[test]
fn a_request_dropped_before_the_worker_reaches_it_is_skipped() {
    let log = Log::default();
    let (release, gate) = mpsc::channel();
    let mut slow = FakeDecoder::new("software").sharing_log(&log);
    slow.open_gate = Some(Arc::new(Mutex::new(gate)));
    let loader = ImageLoader::with_worker(images(), decoders([slow])).unwrap();
    let mut first = loader.decode(bytes(), DecodeOptions::default());
    assert!(first.try_take().is_none());
    let abandoned = loader.decode(bytes(), DecodeOptions::default());
    drop(abandoned);
    release.send(()).unwrap();
    block_on(first).unwrap();
    // Only the first request ever opened; the abandoned one was skipped, not decoded and thrown away.
    let kept = loader.decode(bytes(), DecodeOptions::default());
    release.send(()).unwrap();
    block_on(kept).unwrap();
    let opens = log
        .events()
        .iter()
        .filter(|e| e.starts_with("open"))
        .count();
    assert_eq!(opens, 2);
}

#[test]
fn a_worker_that_dies_resolves_waiting_and_later_requests_with_closed() {
    struct Panics;
    impl valo_codec::Decoder for Panics {
        fn name(&self) -> &'static str {
            "panics"
        }
        fn open<'a>(
            &'a self,
            _: &'a valo_codec::OpenRequest,
        ) -> valo_codec::Decoding<'a, Result<Box<dyn valo_codec::FrameReader>, valo_codec::OpenError>>
        {
            panic!("decoder bug");
        }
    }
    let loader = ImageLoader::with_worker(images(), vec![Box::new(Panics)]).unwrap();
    let first = loader.decode(bytes(), DecodeOptions::default());
    assert!(matches!(block_on(first), Err(DecodeError::Closed)));
    thread::sleep(Duration::from_millis(20));
    let later = loader.decode(bytes(), DecodeOptions::default());
    assert!(matches!(block_on(later), Err(DecodeError::Closed)));
}

#[test]
fn the_worker_sleeps_through_a_decode_that_answers_later_and_wakes_for_the_answer() {
    let (decoder, release) = DelayedDecoder::new("browser");
    let loader = ImageLoader::with_worker(images(), vec![Box::new(decoder)]).unwrap();
    let decoding = loader.decode(bytes(), DecodeOptions::default());
    let releaser = thread::spawn(move || {
        thread::sleep(Duration::from_millis(20));
        release.release();
    });
    let image = block_on(decoding).unwrap();
    assert_eq!(image.size(), [1, 1]);
    releaser.join().unwrap();
}
