//! A scripted decoder for exercising the loader without any real codec.
#![allow(dead_code)]
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;
use valo::{AlphaType, ImageContext, PixelBuffer, PixelFormat, PixelLayout};
use valo_codec::{
    DecodeError, DecodedFrame, Decoder, FramePixels, FrameReader, ImageInfo, OpenError,
    OpenRequest, Repetition,
};

/// One event the fake records, with the thread it happened on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Event {
    pub what: String,
    pub thread: ThreadId,
}

#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<Event>>>);

impl Log {
    fn record(&self, what: impl Into<String>) {
        self.0.lock().unwrap().push(Event {
            what: what.into(),
            thread: thread::current().id(),
        });
    }

    pub fn events(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|e| e.what.clone())
            .collect()
    }

    pub fn threads(&self) -> Vec<ThreadId> {
        self.0.lock().unwrap().iter().map(|e| e.thread).collect()
    }
}

#[derive(Clone)]
pub enum OnOpen {
    Accept,
    Decline(&'static str),
    Fail(DecodeError),
}

/// FakeDecoder opens any bytes according to its script and records what happens to it.
#[derive(Clone)]
pub struct FakeDecoder {
    pub name: &'static str,
    pub on_open: OnOpen,
    pub frame_count: u32,
    pub repetition: Repetition,
    pub size: [u32; 2],
    pub fail_frame: Option<(u32, DecodeError)>,
    /// Blocks `open` until the receiver yields, for racing cancellation against an open.
    pub open_gate: Option<Arc<Mutex<std::sync::mpsc::Receiver<()>>>>,
    pub log: Log,
}

impl FakeDecoder {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            on_open: OnOpen::Accept,
            frame_count: 1,
            repetition: Repetition::Once,
            size: [1, 1],
            fail_frame: None,
            open_gate: None,
            log: Log::default(),
        }
    }

    pub fn frames(mut self, count: u32, repetition: Repetition) -> Self {
        self.frame_count = count;
        self.repetition = repetition;
        self
    }

    pub fn declining(mut self, reason: &'static str) -> Self {
        self.on_open = OnOpen::Decline(reason);
        self
    }

    pub fn failing(mut self, error: DecodeError) -> Self {
        self.on_open = OnOpen::Fail(error);
        self
    }

    pub fn sharing_log(mut self, log: &Log) -> Self {
        self.log = log.clone();
        self
    }
}

impl Decoder for FakeDecoder {
    fn name(&self) -> &'static str {
        self.name
    }

    fn open(&self, request: &OpenRequest) -> Result<Box<dyn FrameReader>, OpenError> {
        self.log.record(format!("open:{}", self.name));
        if let Some(gate) = &self.open_gate {
            gate.lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .expect("test releases the open gate");
        }
        match &self.on_open {
            OnOpen::Decline(reason) => Err(OpenError::Unsupported((*reason).into())),
            OnOpen::Fail(error) => Err(OpenError::Failed(error.clone())),
            OnOpen::Accept => Ok(Box::new(FakeReader {
                decoder: self.clone(),
                size: request.options.fit(self.size),
                next: 0,
                owner: thread::current().id(),
            })),
        }
    }
}

pub struct FakeReader {
    decoder: FakeDecoder,
    size: [u32; 2],
    next: u32,
    owner: ThreadId,
}

impl FrameReader for FakeReader {
    fn info(&self) -> ImageInfo {
        ImageInfo {
            size: self.size,
            frame_count: self.decoder.frame_count,
            repetition: self.decoder.repetition,
        }
    }

    fn next_frame(&mut self) -> Result<DecodedFrame, DecodeError> {
        assert_eq!(
            self.owner,
            thread::current().id(),
            "reader used off its thread"
        );
        let index = self.next;
        self.next = (self.next + 1) % self.decoder.frame_count;
        self.decoder
            .log
            .record(format!("frame:{}:{index}", self.decoder.name));
        if let Some((at, error)) = &self.decoder.fail_frame {
            if *at == index {
                return Err(error.clone());
            }
        }
        Ok(DecodedFrame {
            pixels: FramePixels::Cpu(solid(self.size, [index as u8, 0, 0, 255])),
            duration: Duration::from_millis(10 + u64::from(index)),
        })
    }
}

impl Drop for FakeReader {
    fn drop(&mut self) {
        assert_eq!(
            self.owner,
            thread::current().id(),
            "reader dropped off its thread"
        );
        self.decoder
            .log
            .record(format!("drop:{}", self.decoder.name));
    }
}

/// `solid` is a straight-alpha RGBA buffer of one colour.
pub fn solid(size: [u32; 2], rgba: [u8; 4]) -> PixelBuffer {
    let count = (size[0] * size[1]) as usize;
    PixelBuffer::new(
        PixelLayout::packed(size, PixelFormat::Rgba8, AlphaType::Straight),
        rgba.repeat(count),
    )
    .unwrap()
}

pub fn images() -> ImageContext {
    let (device, queue) = valo_harness::headless_device().expect("headless GPU");
    ImageContext::new(device, queue)
}

pub fn bytes() -> Arc<[u8]> {
    Arc::from([1, 2, 3])
}

pub fn decoders(list: impl IntoIterator<Item = FakeDecoder>) -> Vec<Box<dyn Decoder>> {
    list.into_iter()
        .map(|d| Box::new(d) as Box<dyn Decoder>)
        .collect()
}
