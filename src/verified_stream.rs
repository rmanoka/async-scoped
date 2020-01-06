use futures::Stream;
use std::pin::Pin;
use std::marker::PhantomData;
use std::task::{Context, Poll, Waker};
use std::sync::{Arc, Mutex};
use async_std::sync::RwLock;

/// A stream wrapper that ensures the underlying stream is
/// driven to completion before being dropped. The
/// implementation blocks the thread and drains the stream
/// if dropped before completion.
///
/// It is **not safe** to `forget` a `VerifiedStream` that
/// has not been fully driven.
pub struct VerifiedStream<'a, S: Stream>{
    stream: S,
    pub done: bool,
    pub len: usize,
    lock: Arc<RwLock<bool>>,
    read_wakers: Arc<Mutex<Vec<Waker>>>,
    _marker: PhantomData<&'a ()>,
}

impl<'a, I, S: Stream<Item=I>> VerifiedStream<'a, S> {
    pub fn new(stream: S, len: usize, lock: Arc<RwLock<bool>>, read_wakers: Arc<Mutex<Vec<Waker>>>) -> Self {
        VerifiedStream {
            done: false,
            _marker: PhantomData,
            stream, lock, len, read_wakers,
        }
    }

    fn stream(self: Pin<&mut Self>) -> Pin<&mut S> {
        // Only for projection in `poll_next`.
        unsafe { self.map_unchecked_mut(|o| &mut o.stream) }
    }

    fn done(self: Pin<&mut Self>) -> &mut bool {
        // Only for projection in `poll_next`.
        unsafe { &mut self.get_unchecked_mut().done }
    }

    /// Cancel all futures spawned with cancellation.
    pub async fn cancel(&self) {
        // Mark scope as being cancelled.
        *(self.lock.write().await) = false;

        // At this point, the read_wakers list is stable.
        // No more wakers could be added any more (as the flag is set).
        let mut list = self.read_wakers.lock().unwrap();
        for w in list.iter() {
            w.wake_by_ref();
        }
        list.clear();
    }
}

impl<'a, T: Stream> Drop for VerifiedStream<'a, T> {
    fn drop(&mut self) {
        if !self.done {
            async_std::task::block_on(async {
                // This is the destructor, so it is okay to pin from &mut
                let mut pinned: Pin<&mut Self> = unsafe { Pin::new_unchecked(self) };
                pinned.cancel().await;

                // Await all the futures to be dropped.
                use futures::StreamExt;
                while let Some(_) = pinned.next().await {
                }
            });
        }
    }
}

impl<'a, I, T: Stream<Item=I>> Stream for VerifiedStream<'a, T> {
    type Item = I;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context)
                 -> Poll<Option<Self::Item>> {
        let inner = self.as_mut().stream();
        let poll = inner.poll_next(cx);
        if let Poll::Ready(None) = poll {
            *(self.done()) = true;
        }
        poll
    }
}
