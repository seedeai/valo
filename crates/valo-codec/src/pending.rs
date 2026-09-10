//! The one completion primitive: a result on its way, usable as a future or by polling.
use crate::DecodeError;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

type Outcome<T> = Result<T, DecodeError>;
type BoxedFuture<T> = Pin<Box<dyn Future<Output = Outcome<T>>>>;

/// Pending is a decode result on its way.
///
/// It is a `Future`: `.await` it on any single-threaded executor, or wrap it so its waker also
/// wakes an event loop. A host that runs a frame loop instead calls [`try_take`](Self::try_take)
/// once a frame. Dropping a `Pending` cancels the work if it has not started.
///
/// The type is `!Send` on purpose: the inline loader runs the decode inside `poll`, on the
/// polling thread, and an opened [`Codec`](crate::Codec) belongs to that thread too.
pub struct Pending<T> {
    future: Option<BoxedFuture<T>>,
}

impl<T> Pending<T> {
    /// `inline` runs `job` on the first poll and is ready immediately after.
    pub(crate) fn inline(job: impl FnOnce() -> Outcome<T> + 'static) -> Self
    where
        T: 'static,
    {
        Self::from_future(async move { job() })
    }

    pub(crate) fn from_future(future: impl Future<Output = Outcome<T>> + 'static) -> Self {
        Self {
            future: Some(Box::pin(future)),
        }
    }

    /// `try_take` returns the result if it is ready, without blocking.
    ///
    /// With an inline loader the first call does the decode. Returns `None` while a worker is
    /// still busy.
    ///
    /// # Panics
    /// Panics if called again after it returned `Some`.
    pub fn try_take(&mut self) -> Option<Outcome<T>> {
        let mut context = Context::from_waker(Waker::noop());
        match self.poll_inner(&mut context) {
            Poll::Ready(result) => Some(result),
            Poll::Pending => None,
        }
    }

    fn poll_inner(&mut self, context: &mut Context<'_>) -> Poll<Outcome<T>> {
        let future = self
            .future
            .as_mut()
            .expect("Pending polled after it completed");
        let result = future.as_mut().poll(context);
        if result.is_ready() {
            self.future = None;
        }
        result
    }
}

impl<T> Future for Pending<T> {
    type Output = Outcome<T>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_mut().poll_inner(context)
    }
}

impl<T> std::fmt::Debug for Pending<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("completed", &self.future.is_none())
            .finish()
    }
}

#[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
pub(crate) use shared::{reply, Reply};

/// The cross-thread half: a slot the worker fills and the caller's future waits on.
#[cfg(all(feature = "worker", not(target_arch = "wasm32")))]
mod shared {
    use super::*;
    use std::sync::{Arc, Mutex};

    enum State<T> {
        Waiting(Option<Waker>),
        Ready(Outcome<T>),
        Taken,
        Cancelled,
    }

    struct Slot<T>(Mutex<State<T>>);

    impl<T> Slot<T> {
        fn lock(&self) -> std::sync::MutexGuard<'_, State<T>> {
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        }
    }

    /// `reply` makes a linked pair: the future the caller awaits and the reply a worker fills.
    pub(crate) fn reply<T>() -> (impl Future<Output = Outcome<T>>, Reply<T>) {
        let slot = Arc::new(Slot(Mutex::new(State::Waiting(None))));
        (Awaiting(slot.clone()), Reply(slot))
    }

    struct Awaiting<T>(Arc<Slot<T>>);

    impl<T> Future for Awaiting<T> {
        type Output = Outcome<T>;

        fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
            let mut state = self.0.lock();
            match &mut *state {
                State::Waiting(waker) => {
                    *waker = Some(context.waker().clone());
                    Poll::Pending
                }
                State::Ready(_) => match std::mem::replace(&mut *state, State::Taken) {
                    State::Ready(result) => Poll::Ready(result),
                    _ => unreachable!("matched Ready under the same lock"),
                },
                State::Taken => panic!("decode result taken twice"),
                State::Cancelled => unreachable!("only a dropped future cancels"),
            }
        }
    }

    /// Dropping the future is the cancellation signal a worker reads before starting a job.
    impl<T> Drop for Awaiting<T> {
        fn drop(&mut self) {
            let mut state = self.0.lock();
            if matches!(*state, State::Waiting(_)) {
                *state = State::Cancelled;
            }
        }
    }

    /// Reply is the worker's handle for delivering one result.
    pub(crate) struct Reply<T>(Arc<Slot<T>>);

    impl<T> Reply<T> {
        /// `is_cancelled` is true once the caller stopped waiting; skip the work.
        pub(crate) fn is_cancelled(&self) -> bool {
            matches!(*self.0.lock(), State::Cancelled)
        }

        /// `send` delivers the result and wakes the caller, or hands it back if nobody waits.
        pub(crate) fn send(self, result: Outcome<T>) -> Result<(), Outcome<T>> {
            deliver(&self.0, result)
        }
    }

    /// A reply dropped unsent — its job never ran because the worker went away — resolves the
    /// caller with [`DecodeError::Closed`] rather than leaving it waiting forever.
    impl<T> Drop for Reply<T> {
        fn drop(&mut self) {
            let _ = deliver(&self.0, Err(DecodeError::Closed));
        }
    }

    /// Completes a waiting slot and wakes its poller; a slot nobody waits on refuses the result.
    fn deliver<T>(slot: &Slot<T>, result: Outcome<T>) -> Result<(), Outcome<T>> {
        let mut state = slot.lock();
        let State::Waiting(waker) = &mut *state else {
            return Err(result);
        };
        let waker = waker.take();
        *state = State::Ready(result);
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }
}
