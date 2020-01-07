use std::{
    collections::HashMap,
    future::Future, pin::Pin,
    sync::{Arc, Mutex},
    task::{Poll, Context, Waker}};
use async_std::sync::RwLock;

/// A wrapper for `Future` to make it cancellable from the
/// scope that spawned it. The future may be cancelled by
/// calling `cancel` method or dropping the `Scope`.
pub struct CancellableFuture<I, F: Future<Output=I>, Fu: FnOnce() -> I> {
    id: usize,
    lock: Arc<RwLock<bool>>,
    read_wakers: Arc<Mutex<HashMap<usize, Waker>>>,
    fut: F,
    cancellation: Option<Fu>,
}

impl<I, F: Future<Output=I>, Fu: FnOnce() -> I> CancellableFuture<I, F, Fu> {
    pub fn new(id: usize,
               lock: Arc<RwLock<bool>>,
               read_wakers: Arc<Mutex<HashMap<usize, Waker>>>,
               fut: F, cancellation: Fu) -> Self {
        CancellableFuture{id, lock, read_wakers, fut, cancellation: Some(cancellation)}
    }

    fn future(self: Pin<&mut Self>) -> Pin<&mut F> {
        // Only for projection in `poll`.
        unsafe { self.map_unchecked_mut(|o| &mut o.fut) }
    }

    fn cancellation(self: Pin<&mut Self>) -> &mut Option<Fu> {
        // Only for projection in `poll`.
        unsafe { &mut self.get_unchecked_mut().cancellation }
    }
}

impl<I, F: Future<Output=I>, Fu: FnOnce() -> I> Future
    for CancellableFuture<I, F, Fu>
{
    type Output = I;

    fn poll(self: Pin<&mut Self>, cx: &mut Context)
            -> Poll<Self::Output> {

        let lock = self.lock.clone();
        let read_fut = lock.read();
        futures::pin_mut!(read_fut);
        let polled = read_fut.poll(cx);

        if let Poll::Ready(guard) = polled {
            if !*guard {
                Poll::Ready(self.cancellation().take().unwrap()())
            } else {
                // No spurious move out of the reference involved.
                let this = unsafe { self.get_unchecked_mut() };
                let poll_result = unsafe { Pin::new_unchecked(&mut *this).future().poll(cx) };

                // Add the waker from context into read_wakers list
                let mut map = this.read_wakers.lock().unwrap();
                if poll_result.is_ready() {
                    map.remove(&this.id);
                } else  {
                    map.insert(this.id, cx.waker().clone());
                }

                // Ensure we drop read guard only after adding waker to list
                std::mem::drop(guard);
                poll_result

            }
        } else {
            Poll::Pending
        }

    }
}
