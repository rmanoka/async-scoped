use futures::Stream;
use futures::stream::FuturesOrdered;

use std::pin::Pin;
use std::marker::PhantomData;
use std::task::{Context, Poll};

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
    _marker: PhantomData<&'a ()>,
}

impl<'a, I: std::future::Future>
From<FuturesOrdered<I>> for VerifiedStream<'a, FuturesOrdered<I>> {
    fn from(stream: FuturesOrdered<I>)
            -> VerifiedStream<'a, FuturesOrdered<I>> {
        VerifiedStream {
            len: stream.len(),
            done: false,
            _marker: PhantomData,
            stream,
        }
    }
}

impl<'a, I, S: Stream<Item=I>> VerifiedStream<'a, S> {
    fn stream(self: Pin<&mut Self>) -> Pin<&mut S> {
        // Only for projection in `poll_next`.
        unsafe { self.map_unchecked_mut(|o| &mut o.stream) }
    }

    fn done(self: Pin<&mut Self>) -> &mut bool {
        // Only for projection in `poll_next`.
        unsafe { &mut self.get_unchecked_mut().done }
    }
}

impl<'a, T: Stream> Drop for VerifiedStream<'a, T> {
    fn drop(&mut self) {
        if !self.done {
            // This is the destructor, so it is okay to pin from &mut
            let mut pinned: Pin<&mut Self> = unsafe { Pin::new_unchecked(self) };

            async_std::task::block_on(async {
                use futures::StreamExt;
                while let Some(_) = pinned.next().await {
                }
            });
        }
    }
}

// default impl<'a, T> Drop for VerifiedStream<'a, T> {
//     fn drop(&mut self) {
//         if !self.done {
//             panic!("Scoped future streams must be run to completion");
//         }
//     }
// }

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
