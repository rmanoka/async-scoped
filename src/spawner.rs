//! There you can find traits that are necessary for implementing executor.
//! They are mostly unsafe, as we can't ensure the executor allows intended manipulations
//! without causing UB.
use futures::Future;

pub unsafe trait Spawner<T> {
    type FutureOutput;
    type SpawnHandle: Future<Output = Self::FutureOutput> + Send;
    fn spawn<F: Future<Output = T> + Send + 'static>(&self, f: F) -> Self::SpawnHandle;
}

pub unsafe trait FuncSpawner<T> {
    type FutureOutput;
    type SpawnHandle: Future<Output = Self::FutureOutput> + Send;
    fn spawn_func<F: FnOnce() -> T + Send + 'static>(&self, f: F) -> Self::SpawnHandle;
}

pub unsafe trait Blocker {
    fn block_on<T, F: Future<Output = T>>(&self, f: F) -> T;
}

#[cfg(feature = "use-async-std")]
pub mod use_async_std {
    use super::*;
    use async_std::task::{block_on, spawn, spawn_blocking, JoinHandle};

    #[derive(Default)]
    pub struct AsyncStdSpawner;

    unsafe impl<T: Send + 'static> Spawner<T> for AsyncStdSpawner {
        type FutureOutput = T;
        type SpawnHandle = JoinHandle<T>;

        fn spawn<F: Future<Output = T> + Send + 'static>(&self, f: F) -> Self::SpawnHandle {
            spawn(f)
        }
    }
    unsafe impl<T: Send + 'static> FuncSpawner<T> for AsyncStdSpawner {
        type FutureOutput = T;
        type SpawnHandle = JoinHandle<T>;

        fn spawn_func<F: FnOnce() -> T + Send + 'static>(&self, f: F) -> Self::SpawnHandle {
            spawn_blocking(f)
        }
    }
    unsafe impl Blocker for AsyncStdSpawner {
        fn block_on<T, F: Future<Output = T>>(&self, f: F) -> T {
            block_on(f)
        }
    }
}

#[cfg(feature = "use-tokio")]
pub mod use_tokio {
    use super::*;
    use tokio::{
        runtime::{Handle, Runtime},
        task::{self as tokio_task, block_in_place},
    };

    pub struct TokioSpawner(Option<TokioRuntime>);

    impl Clone for TokioSpawner {
        fn clone(&self) -> Self {
            Self(self.0.as_ref().map(|rt| match rt {
                TokioRuntime::ByHandle(handle) => TokioRuntime::ByHandle(handle.clone()),
                TokioRuntime::Owned(runtime) => TokioRuntime::ByHandle(runtime.handle().clone()),
            }))
        }
    }

    const RUNTIME_INVARIANT_ERR: &str =
        "invariant: runtime must be available during the spawner's lifetime";

    impl Drop for TokioSpawner {
        /// Graceful shutdown owned runtime.
        fn drop(&mut self) {
            if let TokioRuntime::Owned(rt) = self.0.take().expect(RUNTIME_INVARIANT_ERR) {
                rt.shutdown_background()
            }
        }
    }

    impl TokioSpawner {
        pub fn new(rt_handle: Handle) -> Self {
            Self(Some(TokioRuntime::ByHandle(rt_handle)))
        }

        fn handle(&self) -> &Handle {
            match &self.0.as_ref().expect(RUNTIME_INVARIANT_ERR) {
                TokioRuntime::ByHandle(handle) => handle,
                TokioRuntime::Owned(runtime) => runtime.handle(),
            }
        }
    }

    /// Variants of supplied tokio runtime.
    /// Is needed because runtime can be either passed or created.
    enum TokioRuntime {
        /// User provides its own runtime, we'll refer to it by handle.
        ByHandle(Handle),
        /// We've created our own ad-hoc runtime, so we'll own it.
        Owned(Runtime),
    }

    // By default, `TokioSpawner` operates on globally available runtime.
    // Ad-hoc runtime would only be created if it is not available globally.
    // Newly created runtime would be destroyed when spawner is gone.
    // That is needed because running runtime would prevent program from stopping.
    impl Default for TokioSpawner {
        fn default() -> Self {
            if let Ok(handle) = Handle::try_current() {
                return Self(Some(TokioRuntime::ByHandle(handle)));
            }
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            Self(Some(TokioRuntime::Owned(runtime)))
        }
    }

    unsafe impl<T: Send + 'static> Spawner<T> for TokioSpawner {
        type FutureOutput = Result<T, tokio_task::JoinError>;
        type SpawnHandle = tokio_task::JoinHandle<T>;

        fn spawn<F: Future<Output = T> + Send + 'static>(&self, f: F) -> Self::SpawnHandle {
            self.handle().spawn(f)
        }
    }

    unsafe impl<T: Send + 'static> FuncSpawner<T> for TokioSpawner {
        type FutureOutput = Result<T, tokio_task::JoinError>;
        type SpawnHandle = tokio_task::JoinHandle<T>;

        fn spawn_func<F: FnOnce() -> T + Send + 'static>(&self, f: F) -> Self::SpawnHandle {
            self.handle().spawn_blocking(f)
        }
    }

    unsafe impl Blocker for TokioSpawner {
        fn block_on<T, F: Future<Output = T>>(&self, f: F) -> T {
            let result = block_in_place(|| match self.0.as_ref().expect(RUNTIME_INVARIANT_ERR) {
                TokioRuntime::ByHandle(handle) => handle.block_on(f),
                // if runtime is owned, `block_on` must be called directly on it,
                // not via it's handle. Otherwise, future won't be able to run IO-tasks.
                // see `block_on` docs for more info.
                TokioRuntime::Owned(runtime) => runtime.block_on(f),
            });
            result
        }
    }
}
