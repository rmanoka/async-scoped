use std::sync::Mutex;
use std::task::{Poll, Waker, Context};
use std::pin::Pin;
use std::future::Future;
use async_std::sync::RwLock;
use slab::Slab;

pub struct Cancellation {
    flag: RwLock<bool>,
    read_wakers: Mutex<Slab<Waker>>,
}

impl Cancellation {
    pub fn new() -> Self {
        Cancellation {
            flag: RwLock::new(false),
            read_wakers: Mutex::new(Slab::new()),
        }
    }

    /// Trigger cancellation: set lock to false and wake all
    /// futures registered with us.
    pub async fn cancel(&self) {
        // Mark scope as being cancelled.
        let mut lock = self.flag.write().await;
        if *lock { return; }
        *lock = true;

        // At this point, the read_wakers list is stable.
        // No more wakers could be added any more (as the flag is set).
        let mut list = self.read_wakers.lock().unwrap();
        for v in list.drain() {
            v.wake();
        }
    }

    /// Poll a future if cancellation has not happened. If
    /// polled, the poll result is returned; otherwise, the
    /// cancellation has been triggerred, and this method
    /// returns None.
    pub fn poll_future<I, F: Future<Output=I>>(
        &self,
        mut key: Option<usize>,
        fut: Pin<&mut F>, cx: &mut Context,
    ) -> Option<(Poll<I>, Option<usize>)> {

        if let Some(guard) = self.flag.try_read() {
            if *guard {
                let poll_result = fut.poll(cx);

                // Add the waker from context into read_wakers list
                let mut map = self.read_wakers.lock().unwrap();
                if poll_result.is_ready() {
                    // This future is ready
                    if let Some(id) = key {
                        map.remove(id);
                        key = None;
                    }
                } else  {
                    // Register cancellation wake
                    if let Some(id) = key {
                        if let Some(slot) = map.get_mut(id) {
                            *slot = cx.waker().clone();
                        } else {
                            // If we have a key, it must be valid.
                            unreachable!();
                        }
                    } else {
                        key = Some(map.insert(cx.waker().clone()));
                    }
                }

                // Ensure we drop read guard only after adding waker to list
                std::mem::drop(map);
                std::mem::drop(guard);
                return Some((poll_result, key));
            }
        }
        // Either already cancelled, or being cancelled.
        None

    }
}
