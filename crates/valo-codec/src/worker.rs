//! The worker thread: it runs the same decode service as the inline path, but owns the opened
//! readers, so decoder state never crosses a thread after it is created.
use crate::pending::{reply, Reply};
use crate::service::DecodeService;
use crate::{Codec, DecodeError, DecodeOptions, Frame, FrameReader, ImageInfo, Pending};
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use valo::Image;

/// Names one reader held by the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ReaderId(u64);

/// Opened is what the worker reports back for an open: the caller builds the codec from it.
struct Opened {
    id: ReaderId,
    info: ImageInfo,
}

enum Job {
    Decode {
        encoded: Arc<[u8]>,
        options: DecodeOptions,
        reply: Reply<Image>,
    },
    Open {
        encoded: Arc<[u8]>,
        options: DecodeOptions,
        reply: Reply<Opened>,
    },
    Next {
        reader: ReaderId,
        mipmaps: bool,
        reply: Reply<Frame>,
    },
    Close(ReaderId),
}

/// Worker is the caller's handle to the decode thread; clones share it.
#[derive(Clone)]
pub(crate) struct Worker {
    sender: mpsc::Sender<Job>,
}

impl Worker {
    pub(crate) fn spawn(service: Arc<DecodeService>) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::Builder::new()
            .name("valo image decode".into())
            .spawn(move || Thread::new(service).run(receiver))?;
        Ok(Self { sender })
    }

    pub(crate) fn decode(&self, encoded: Arc<[u8]>, options: DecodeOptions) -> Pending<Image> {
        let (awaiting, reply) = reply();
        self.submit(Job::Decode {
            encoded,
            options,
            reply,
        });
        Pending::from_future(awaiting)
    }

    pub(crate) fn open(&self, encoded: Arc<[u8]>, options: DecodeOptions) -> Pending<Codec> {
        let (awaiting, reply) = reply();
        self.submit(Job::Open {
            encoded,
            options,
            reply,
        });
        let sender = self.sender.clone();
        Pending::from_future(async move {
            let opened = awaiting.await?;
            let remote = RemoteReader {
                id: opened.id,
                mipmaps: options.mipmaps,
                sender,
            };
            Ok(Codec::remote(opened.info, options.mipmaps, remote))
        })
    }

    /// A job the thread never receives drops its reply, which resolves the caller with `Closed`.
    fn submit(&self, job: Job) {
        let _ = self.sender.send(job);
    }
}

/// RemoteReader is a codec's handle to a reader living on the worker.
pub(crate) struct RemoteReader {
    id: ReaderId,
    mipmaps: bool,
    sender: mpsc::Sender<Job>,
}

impl RemoteReader {
    pub(crate) fn next_frame(&self) -> Pending<Frame> {
        let (awaiting, reply) = reply();
        let _ = self.sender.send(Job::Next {
            reader: self.id,
            mipmaps: self.mipmaps,
            reply,
        });
        Pending::from_future(awaiting)
    }
}

impl Drop for RemoteReader {
    fn drop(&mut self) {
        let _ = self.sender.send(Job::Close(self.id));
    }
}

/// The thread's state: the service plus every reader opened and not yet closed.
struct Thread {
    service: Arc<DecodeService>,
    readers: HashMap<ReaderId, Box<dyn FrameReader>>,
    next_id: u64,
}

impl Thread {
    fn new(service: Arc<DecodeService>) -> Self {
        Self {
            service,
            readers: HashMap::new(),
            next_id: 0,
        }
    }

    /// Runs until every sender is gone: the loader and all codecs it opened.
    fn run(mut self, receiver: mpsc::Receiver<Job>) {
        while let Ok(job) = receiver.recv() {
            self.run_job(job);
        }
    }

    fn run_job(&mut self, job: Job) {
        match job {
            Job::Decode {
                encoded,
                options,
                reply,
            } => {
                if !reply.is_cancelled() {
                    let _ = reply.send(self.service.decode_still(encoded, options));
                }
            }
            Job::Open {
                encoded,
                options,
                reply,
            } => {
                if !reply.is_cancelled() {
                    self.open(encoded, options, reply);
                }
            }
            Job::Next {
                reader,
                mipmaps,
                reply,
            } => {
                if !reply.is_cancelled() {
                    let _ = reply.send(self.next_frame(reader, mipmaps));
                }
            }
            Job::Close(id) => {
                self.readers.remove(&id);
            }
        }
    }

    fn open(&mut self, encoded: Arc<[u8]>, options: DecodeOptions, reply: Reply<Opened>) {
        let reader = match self.service.open(encoded, options) {
            Ok(reader) => reader,
            Err(error) => {
                let _ = reply.send(Err(error));
                return;
            }
        };
        let id = ReaderId(self.next_id);
        self.next_id += 1;
        let info = reader.info();
        self.readers.insert(id, reader);
        // The caller gave up while we were opening: nothing will ever close this reader, so
        // release it here rather than leak it.
        if reply.send(Ok(Opened { id, info })).is_err() {
            self.readers.remove(&id);
        }
    }

    fn next_frame(&mut self, id: ReaderId, mipmaps: bool) -> Result<Frame, DecodeError> {
        let reader = self.readers.get_mut(&id).ok_or(DecodeError::Closed)?;
        self.service.next_frame(reader.as_mut(), mipmaps)
    }
}
