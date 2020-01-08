use std::{
    future::Future, pin::Pin, sync::Arc,
    task::{Poll, Context}};
use pin_project::pin_project;
use crate::Cancellation;

/// A wrapper for `Future` to make it cancellable from the
/// scope that spawned it. The future may be cancelled by
/// calling `cancel` method or dropping the `Scope`.
#[pin_project]
pub struct CancellableFuture<I, F: Future<Output=I>, Fu: FnOnce() -> I> {
    key: Option<usize>,
    cancellation: Arc<Cancellation>,
    default: Option<Fu>,
    #[pin]
    fut: F,
}

impl<I, F: Future<Output=I>, Fu: FnOnce() -> I> CancellableFuture<I, F, Fu> {
    pub fn new(cancellation: Arc<Cancellation>,
               fut: F, default: Fu) -> Self {
        CancellableFuture{key: None, cancellation, fut, default: Some(default)}
    }
}

impl<I, F: Future<Output=I>, Fu: FnOnce() -> I> Future
    for CancellableFuture<I, F, Fu>
{

    type Output = I;
    fn poll(self: Pin<&mut Self>, cx: &mut Context)
            -> Poll<Self::Output> {

        let this = self.project();

        if let Some((result, new_key)) = this.cancellation.poll_future(*this.key, this.fut, cx) {
            *this.key = new_key;
            result
        } else {
            Poll::Ready(this.default.take().unwrap()())
        }
    }

}
