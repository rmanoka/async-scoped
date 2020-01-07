use std::{
    collections::HashMap,
    future::Future, pin::Pin,
    sync::{Arc, Mutex},
    task::{Poll, Context, Waker}};
use async_std::sync::RwLock;
use pin_project::pin_project;

/// A wrapper for `Future` to make it cancellable from the
/// scope that spawned it. The future may be cancelled by
/// calling `cancel` method or dropping the `Scope`.
#[pin_project]
pub struct CancellableFuture<I, F: Future<Output=I>, Fu: FnOnce() -> I> {
    id: usize,
    lock: Arc<RwLock<bool>>,
    read_wakers: Arc<Mutex<HashMap<usize, Waker>>>,
    cancellation: Option<Fu>,
    #[pin]
    fut: F,
}

impl<I, F: Future<Output=I>, Fu: FnOnce() -> I> CancellableFuture<I, F, Fu> {
    pub fn new(id: usize,
               lock: Arc<RwLock<bool>>,
               read_wakers: Arc<Mutex<HashMap<usize, Waker>>>,
               fut: F, cancellation: Fu) -> Self {
        CancellableFuture{id, lock, read_wakers, fut, cancellation: Some(cancellation)}
    }
}

impl<I, F: Future<Output=I>, Fu: FnOnce() -> I> Future
    for CancellableFuture<I, F, Fu>
{
    type Output = I;

    fn poll(self: Pin<&mut Self>, cx: &mut Context)
            -> Poll<Self::Output> {

        let this = self.project();

        let read_fut = this.lock.read();
        futures::pin_mut!(read_fut);
        let polled = read_fut.poll(cx);

        if let Poll::Ready(guard) = polled {
            if !*guard {
                Poll::Ready(this.cancellation.take().unwrap()())
            } else {
                let poll_result = this.fut.poll(cx);

                // Add the waker from context into read_wakers list
                let mut map = this.read_wakers.lock().unwrap();
                if poll_result.is_ready() {
                    map.remove(this.id);
                } else  {
                    map.insert(*this.id, cx.waker().clone());
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
