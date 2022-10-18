use futures::Future;

pub trait Spawner<T> {
    type FutureOutput;
    type SpawnHandle: Future<Output=Self::FutureOutput> + Send;
    fn spawn<F: Future<Output=T> + Send + 'static>(f: F) -> Self::SpawnHandle;
}

pub trait FuncSpawner<T> {
    type FutureOutput;
    type SpawnHandle: Future<Output = Self::FutureOutput> + Send;
    fn spawn_func<F: FnOnce() -> T + Send + 'static>(f: F) -> Self::SpawnHandle;
}

pub trait Blocker {
    fn block_on<T, F: Future<Output=T>>(f: F) -> T;
}

#[cfg(feature = "use-async-std")]
pub mod use_async_std {
    use super::*;
    use async_std::task::{spawn, block_on, spawn_blocking, JoinHandle};
    pub struct AsyncStd;

    impl<T: Send + 'static> Spawner<T> for AsyncStd {
        type FutureOutput = T;
        type SpawnHandle = JoinHandle<T>;

        fn spawn<F: Future<Output=T> + Send + 'static>(f: F) -> Self::SpawnHandle {
            spawn(f)
        }
    }
    impl<T: Send + 'static> FuncSpawner<T> for AsyncStd {
        type FutureOutput = T;
        type SpawnHandle = JoinHandle<T>;

        fn spawn_func<F: FnOnce() -> T + Send + 'static>(f: F) -> Self::SpawnHandle {
            spawn_blocking(f)
        }
    }
    impl Blocker for AsyncStd {
        fn block_on<T, F: Future<Output=T>>(f: F) -> T {
            block_on(f)
        }
    }
}


#[cfg(feature = "use-tokio")]
pub mod use_tokio {
    use super::*;
    use tokio::task as tokio_task;
    pub struct Tokio;

    impl<T: Send + 'static> Spawner<T> for Tokio {
        type FutureOutput = Result<T, tokio_task::JoinError>;
        type SpawnHandle = tokio_task::JoinHandle<T>;

        fn spawn<F: Future<Output=T> + Send + 'static>(f: F) -> Self::SpawnHandle {
            tokio_task::spawn(f)
        }
    }

    impl<T: Send + 'static> FuncSpawner<T> for Tokio {
        type FutureOutput = Result<T, tokio_task::JoinError>;
        type SpawnHandle = tokio_task::JoinHandle<T>;

        fn spawn_func<F: FnOnce() -> T + Send + 'static>(f: F) -> Self::SpawnHandle {
            tokio_task::spawn_blocking(f)
        }
    }

    impl Blocker for Tokio {
        fn block_on<T, F: Future<Output=T>>(f: F) -> T {
            tokio_task::block_in_place(|| {
                tokio::runtime::Builder::new_current_thread()
                    .build()
                    .unwrap()
                    .block_on(f)
            })
        }
    }
}
