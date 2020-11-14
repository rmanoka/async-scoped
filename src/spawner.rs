use futures::Future;

pub trait Spawner<T> {
    type SpawnHandle: Future<Output=T> + Send;
    fn spawn<F: Future<Output=T> + Send + 'static>(f: F) -> Self::SpawnHandle;
}

pub trait Blocker {
    fn block_on<T, F: Future<Output=T>>(f: F) -> T;
}

#[cfg(feature = "use-async-std")]
pub mod use_async_std {
    use super::*;
    use async_std::task::{spawn, block_on, JoinHandle};
    pub struct AsyncStd;

    impl<T: Send + 'static> Spawner<T> for AsyncStd {
        type SpawnHandle = JoinHandle<T>;

        fn spawn<F: Future<Output=T> + Send + 'static>(f: F) -> Self::SpawnHandle {
            spawn(f)
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
    use std::pin::Pin;

    impl<T: Send + 'static> Spawner<T> for Tokio {
        type SpawnHandle = Pin<Box<dyn Future<Output=T> + Send>>;

        fn spawn<F: Future<Output=T> + Send + 'static>(f: F) -> Self::SpawnHandle {
            let handle = tokio_task::spawn(f);
            Box::pin(async {
                handle.await.unwrap()
            })
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
