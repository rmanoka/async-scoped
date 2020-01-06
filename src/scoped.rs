use std::task::Poll;
use std::pin::Pin;
use std::task::Context;
use futures::Stream;
use futures::{Future, FutureExt};
use futures::future::BoxFuture;
use futures::stream::FuturesOrdered;
use async_std::task::JoinHandle;
use async_std::sync::RwLock;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::task::Waker;
use std::collections::HashMap;

/// A scope to allow controlled spawning of non 'static
/// futures.
pub struct Scope<'a, T: Send + 'static> {
    done: bool,
    len: usize,
    lock: Arc<RwLock<bool>>,
    read_wakers: Arc<Mutex<HashMap<usize, Waker>>>,
    futs: FuturesOrdered<JoinHandle<T>>,
    _marker: PhantomData<fn(&'a  ())>
}

impl<'a, T: Send + 'static> Scope<'a, T> {
    /// Create a Scope object.
    ///
    /// This function is unsafe as `futs` may hold futures
    /// which have to be manually driven to completion.
    pub unsafe fn create() -> Self {
        Scope{
            done: false,
            len: 0,
            lock: Arc::new(RwLock::new(true)),
            read_wakers: Arc::new(Mutex::new(HashMap::new())),
            futs: FuturesOrdered::new(),
            _marker: PhantomData,
        }
    }

    /// Spawn a future with `async_std::task::spawn`. The
    /// future is expected to be driven to completion before
    /// 'a expires.
    #[inline] pub fn spawn<F: Future<Output=T> + Send + 'a>(&mut self, f: F) {
        let handle = async_std::task::spawn(unsafe {
            std::mem::transmute::<_, BoxFuture<'static, T>>(f.boxed())
        });
        self.futs.push(handle);
        self.len += 1;
    }

    /// Spawn a cancellable future with `async_std::task::spawn`
    ///
    /// The future is cancelled if the `Scope` is dropped
    /// pre-maturely. It can also be cancelled by explicitly
    /// calling (and awaiting) the `cancel` method.
    pub fn spawn_cancellable<F: Future<Output=T> + Send + 'a,
                             Fu: FnOnce() -> T + Send + 'a>(&mut self, f: F, cancellation: Fu) {
        use super::CancellableFuture;
        self.spawn(CancellableFuture::new(self.len, self.lock.clone(), self.read_wakers.clone(), f, cancellation))
    }

    /// Cancel all futures spawned with cancellation.
    pub async fn cancel(&self) {
        // Mark scope as being cancelled.
        let mut lock = self.lock.write().await;
        if !*lock { return; }
        *lock = false;

        // At this point, the read_wakers list is stable.
        // No more wakers could be added any more (as the flag is set).
        let mut list = self.read_wakers.lock().unwrap();
        for w in list.iter() {
            w.1.wake_by_ref();
        }
        list.clear();
    }

    fn stream(self: Pin<&mut Self>) -> Pin<&mut FuturesOrdered<JoinHandle<T>>> {
        // Only for projection in `poll_next`.
        unsafe { self.map_unchecked_mut(|o| &mut o.futs) }
    }

    fn done(self: Pin<&mut Self>) -> &mut bool {
        // Only for projection in `poll_next`.
        unsafe { &mut self.get_unchecked_mut().done }
    }

    /// Total number of futures spawned in this scope.
    pub fn len(&self) -> usize { self.len }
}

impl<'a, T: Send + 'static> Stream for Scope<'a, T> {
    type Item = T;

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

impl<'a, T: Send + 'static> Drop for Scope<'a, T> {
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
