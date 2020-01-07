use std::task::{Poll, Context, Waker};
use std::pin::Pin;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

use futures::{Stream, Future, FutureExt};
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;

use async_std::task::JoinHandle;
use async_std::sync::RwLock;

use pin_project::{pin_project, pinned_drop};

/// A scope to allow controlled spawning of non 'static
/// futures. Futures can be spawned using `spawn` or
/// `spawn_cancellable` methods.
///
/// # Safety
///
/// This type uses `Drop` implementation to guarantee
/// safety. It is not safe to forget this object unless it
/// is driven to completion.
#[pin_project(PinnedDrop)]
pub struct Scope<'a, T> {
    done: bool,
    len: usize,
    remaining: usize,
    lock: Arc<RwLock<bool>>,
    read_wakers: Arc<Mutex<HashMap<usize, Waker>>>,
    #[pin]
    futs: FuturesUnordered<JoinHandle<T>>,
    _marker: PhantomData<fn(&'a ())>
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
            remaining: 0,
            lock: Arc::new(RwLock::new(true)),
            read_wakers: Arc::new(Mutex::new(HashMap::new())),
            futs: FuturesUnordered::new(),
            _marker: PhantomData,
        }
    }

    /// Spawn a future with `async_std::task::spawn`. The
    /// future is expected to be driven to completion before
    /// 'a expires.
    pub fn spawn<F: Future<Output=T> + Send + 'a>(&mut self, f: F) {
        let handle = async_std::task::spawn(unsafe {
            std::mem::transmute::<_, BoxFuture<'static, T>>(f.boxed())
        });
        self.futs.push(handle);
        self.len += 1;
        self.remaining += 1;
    }

    /// Spawn a cancellable future with `async_std::task::spawn`
    ///
    /// The future is cancelled if the `Scope` is dropped
    /// pre-maturely. It can also be cancelled by explicitly
    /// calling (and awaiting) the `cancel` method.
    #[inline]
    pub fn spawn_cancellable<F: Future<Output=T> + Send + 'a,
                             Fu: FnOnce() -> T + Send + 'a>(&mut self, f: F, cancellation: Fu) {
        use super::CancellableFuture;
        self.spawn(CancellableFuture::new(self.len, self.lock.clone(), self.read_wakers.clone(), f, cancellation))
    }
}

impl<'a, T> Scope<'a, T> {
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

    /// Total number of futures spawned in this scope.
    pub fn len(&self) -> usize { self.len }

    /// Number of futures remaining in this scope.
    pub fn remaining(&self) -> usize { self.remaining }

    /// A slighly optimized `collect` on the stream. Also
    /// useful when we can not move out of self.
    pub async fn collect(&mut self) -> Vec<T> {
        let mut proc_outputs = Vec::with_capacity(self.remaining);

        use futures::StreamExt;
        while let Some(item) = self.next().await {
            proc_outputs.push(item);
        }

        proc_outputs
    }
}

impl<'a, T> Stream for Scope<'a, T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context)
                 -> Poll<Option<Self::Item>> {

        let this = self.project();
        let poll = this.futs.poll_next(cx);
        if let Poll::Ready(None) = poll {
            *this.done = true;
        } else if poll.is_ready() {
            *this.remaining -= 1;
        }
        poll

    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

#[pinned_drop]
impl<'a, T> PinnedDrop for Scope<'a, T> {
    fn drop(mut self: Pin<&mut Self>) {
        if !self.done {
            async_std::task::block_on(async {
                self.cancel().await;
                self.collect().await;
            });
        }
    }
}
