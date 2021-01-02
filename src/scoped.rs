use std::marker::PhantomData;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::stream::FuturesUnordered;
use futures::{Future, Stream};

cfg_any_spawner!{
    use std::sync::Arc;
    use crate::Cancellation;
}

use pin_project::*;

use crate::spawner::*;

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
pub struct Scope<'a, T, Sp: Spawner<T> + Blocker> {
    done: bool,
    len: usize,
    remaining: usize,
    #[cfg(any(feature = "use-async-std", feature = "use-tokio"))]
    cancellation: Arc<Cancellation>,
    #[pin]
    futs: FuturesUnordered<Sp::SpawnHandle>,

    // Future proof against variance changes
    _marker: PhantomData<fn(&'a ()) -> &'a ()>,
    _spawn_marker: PhantomData<Sp>,
}

impl<'a, T: Send + 'static, Sp: Spawner<T> + Blocker> Scope<'a, T, Sp> {
    /// Create a Scope object.
    ///
    /// This function is unsafe as `futs` may hold futures
    /// which have to be manually driven to completion.
    pub unsafe fn create() -> Self {
        Scope {
            done: false,
            len: 0,
            remaining: 0,
            #[cfg(any(feature = "use-async-std", feature = "use-tokio"))]
            cancellation: Arc::new(Cancellation::new()),
            futs: FuturesUnordered::new(),
            _marker: PhantomData,
            _spawn_marker: PhantomData,
        }
    }

    /// Spawn a future with `async_std::task::spawn`. The
    /// future is expected to be driven to completion before
    /// 'a expires.
    pub fn spawn<F: Future<Output = T> + Send + 'a>(&mut self, f: F) {
        let handle = Sp::spawn(unsafe {
            std::mem::transmute::<_, Pin<Box<dyn Future<Output = T> + Send>>>(
                Box::pin(f) as Pin<Box<dyn Future<Output = T>>>
            )
        });
        self.futs.push(handle);
        self.len += 1;
        self.remaining += 1;
    }

    cfg_any_spawner!{
        /// Spawn a cancellable future with `async_std::task::spawn`
        ///
        /// The future is cancelled if the `Scope` is dropped
        /// pre-maturely. It can also be cancelled by explicitly
        /// calling (and awaiting) the `cancel` method.
        #[inline]
        pub fn spawn_cancellable<F: Future<Output = T> + Send + 'a, Fu: FnOnce() -> T + Send + 'a>(
            &mut self,
            f: F,
            default: Fu,
        ) {
            self.spawn(crate::CancellableFuture::new(
                self.cancellation.clone(),
                f,
                default,
            ))
        }
    }
}

impl<'a, T, Sp: Spawner<T> + Blocker> Scope<'a, T, Sp> {
    cfg_any_spawner!{
        /// Cancel all futures spawned with cancellation.
        #[inline]
        pub async fn cancel(&self) {
            self.cancellation.cancel().await;
        }
    }

    /// Total number of futures spawned in this scope.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Number of futures remaining in this scope.
    #[inline]
    pub fn remaining(&self) -> usize {
        self.remaining
    }

    /// A slighly optimized `collect` on the stream. Also
    /// useful when we can not move out of self.
    pub async fn collect(&mut self) -> Vec<Sp::FutureOutput> {
        let mut proc_outputs = Vec::with_capacity(self.remaining);

        use futures::StreamExt;
        while let Some(item) = self.next().await {
            proc_outputs.push(item);
        }

        proc_outputs
    }
}

impl<'a, T, Sp: Spawner<T> + Blocker> Stream for Scope<'a, T, Sp> {
    type Item = Sp::FutureOutput;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
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

cfg_any_spawner! {
    #[pinned_drop]
    impl<'a, T, Sp: Spawner<T> + Blocker> PinnedDrop for Scope<'a, T, Sp> {
        fn drop(mut self: Pin<&mut Self>) {
            if !self.done {
                <Sp as Blocker>::block_on(async {
                    self.cancel().await;
                    self.collect().await;
                });
            }
        }
    }
}

cfg_no_spawner! {
    #[pinned_drop]
    impl<'a, T, Sp: Spawner<T> + Blocker> PinnedDrop for Scope<'a, T, Sp> {
        fn drop(mut self: Pin<&mut Self>) {
            if !self.done {
                <Sp as Blocker>::block_on(async {
                    self.collect().await;
                });
            }
        }
    }
}
