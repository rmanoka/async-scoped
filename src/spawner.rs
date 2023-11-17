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
    use tokio::{runtime::Handle, task as tokio_task};

    pub struct TokioSpawner(Handle);

    impl TokioSpawner {
        pub fn new(rt_handle: Handle) -> Self {
            Self(rt_handle)
        }
    }

    // By default, `TokioSpawner` operates on global runtime.
    impl Default for TokioSpawner {
        fn default() -> Self {
            Self(Handle::current())
        }
    }

    unsafe impl<T: Send + 'static> Spawner<T> for TokioSpawner {
        type FutureOutput = Result<T, tokio_task::JoinError>;
        type SpawnHandle = tokio_task::JoinHandle<T>;

        fn spawn<F: Future<Output = T> + Send + 'static>(&self, f: F) -> Self::SpawnHandle {
            self.0.spawn(f)
        }
    }

    unsafe impl<T: Send + 'static> FuncSpawner<T> for TokioSpawner {
        type FutureOutput = Result<T, tokio_task::JoinError>;
        type SpawnHandle = tokio_task::JoinHandle<T>;

        fn spawn_func<F: FnOnce() -> T + Send + 'static>(&self, f: F) -> Self::SpawnHandle {
            self.0.spawn_blocking(f)
        }
    }

    unsafe impl Blocker for TokioSpawner {
        fn block_on<T, F: Future<Output = T>>(&self, f: F) -> T {
            tokio_task::block_in_place(|| self.0.block_on(f))
        }
    }
}
