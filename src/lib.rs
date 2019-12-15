//! Enables controlled spawning of non-`'static` futures when
//! using the [async-std][async_std] executor.
//!
//! ## Motivation
//!
//! Executors like async_std, tokio, etc. support spawning
//! `'static` futures onto a thread-pool. However, it is
//! often useful to spawn a stream of futures that may not
//! all be `'static`.
//!
//! While the future combinators such as [`for_each_concurrent`][for_each_concurrent]
//! offer concurrency, they are bundled as a single [`Task`][Task]
//! structure by the executor, and hence are not driven
//! parallelly.
//!
//! ## Scope API
//!
//! We propose an API similar to [`crossbeam::scope`](crossbeam::scope) to allow
//! controlled spawning of futures that are not `'static`. The
//! key function is:
//!
//! ``` rust, ignore
//! pub unsafe fn scope<'a, T: Send + 'static,
//!              F: FnOnce(&mut Scope<'a, T>)>(f: F)
//!              -> impl Stream {
//!     // ...
//! }
//! ```
//!
//! This function is used as follows:
//!
//! ``` rust
//! #[async_std::test]
//! async fn scoped_futures() {
//!     let not_copy = String::from("hello world!");
//!     let not_copy_ref = &not_copy;
//!     let (foo, outputs) = crate::scope_and_collect!(|s| {
//!         for _ in 0..10 {
//!             let proc = || async {
//!                 assert_eq!(not_copy_ref, "hello world!");
//!             };
//!             s.spawn(proc());
//!         }
//!         42
//!     });
//!     assert_eq!(foo, 42);
//!     assert_eq!(outputs.len(), 10);
//! }
//! ```
//!
//! ## Safety Considerations
//!
//! The [`scope`][scope] API provided in this crate is
//! inherently unsafe. Here, we list the key reasons for
//! unsafety, towards identifying a safe usage (facilitated
//! by [`scope_and_collect!`][scope_and_collect!] macro).
//!
//! 1. Since safe Rust allows [`forget`][forget]-ting the
//!    returned [`Stream`][Stream], the onus of actually
//!    driving it to completion is on the user. Thus the
//!    [`scope`][scope] function is inherently unsafe.
//!
//! 2. The spawned future _must be_ dropped immediately
//!    after completion as it may contain references that
//!    are soon to expire. This is the behaviour of
//!    [async-std][async-std], and the safety of the API
//!    here relies upon it.
//!
//! 3. The parent future hosting the values referred to by
//!    the spawned futures must not be moved while the
//!    spawned futures are running. To the best of our
//!    understanding, the compiler should deduce that an
//!    async function that uses
//!    [`scope_and_collect!`][scope_and_collect!] (or
//!    equivalently [`scope`][scope] and awaiting the output
//!    within the function) is [`!Unpin`][!Unpin].
//!
//! 4. The [`Task`][Task] containing the parent Stream must
//!    not drop the future before completion. This is
//!    generally true, but must hold even if another
//!    associated future (running in the same Task) panics!
//!    Again, we hope this will not happen within
//!    [`poll`][poll] because of above [`!Unpin`][!Unpin]
//!    considerations.
//!
//! ## Implementation
//!
//! Our current implementation simply uses _unsafe_ glue to
//! actually spawn the futures. Then, it records the
//! lifetime of the futures in the returned
//! [`Stream`][Stream] object.
//!
//! Currently, for soundness, we simply panic! if the stream
//! is dropped before it is fully driven. Another option
//! (not implemented here), may be to drive the stream using
//! a current-thread executor inside the [`Drop`][Drop]
//! impl.
//!
//! Unfortunately, since the [`std::mem::forget`][forget]
//! method is safe, the API here is _inherently unsafe_.
//!
//! [async-std]: async_std
//! [poll]: std::futures::Future::poll
//! [Task]: std::task::Task
//! [forget]: std::mem::forget
//! [Stream]: futures::Stream
//! [for_each_concurrent]: futures::StreamExt::for_each_concurrent

use futures::{Future, FutureExt, Stream};
use futures::future::BoxFuture;
use futures::stream::FuturesOrdered;
use async_std::task::JoinHandle;

use std::pin::Pin;
use std::marker::PhantomData;
use std::task::{Context, Poll};

/// A scope to allow controlled spawning of non 'static
/// futures.
pub struct Scope<'a, T: Send + 'static> {
    futs: FuturesOrdered<JoinHandle<T>>,
    _marker: PhantomData<fn(&'a ())>
}

impl<'a, T: Send + 'static> Scope<'a, T> {
    /// Create a Scope object.
    ///
    /// This function is unsafe as `futs` may hold futures
    /// which have to be manually driven to completion.
    pub unsafe fn create() -> Self {
        Scope{
            futs: FuturesOrdered::new(),
            _marker: PhantomData,
        }
    }

    /// Spawn a future with `async_std::task::spawn`
    ///
    /// The future is expected to be driven to completion
    /// before 'a expires. Otherwise, the stream returned by
    /// `scope` function will panic!
    ///
    /// # Safety
    ///
    /// This function is safe as it is unsafe to create a
    /// `Scope` object. The creator of the object is
    /// expected to enforce the lifetime guarantees.
    #[inline] pub fn spawn<F: Future<Output=T> + Send + 'a>(&mut self, f: F) {
        let handle = async_std::task::spawn(unsafe {
            std::mem::transmute::<_, BoxFuture<'static, T>>(f.boxed())
        });
        self.futs.push(handle);
    }
}

/// Creates a `Scope` to spawn non-'static futures. The
/// function is called with a block which takes an `&mut
/// Scope`. The `spawn` method on this arg. can be used to
/// spawn "local" futures.
///
/// # Returns
///
/// The returned value implements `Stream` and is expected
/// to be driven completely before being dropped. The values
/// returned are either the output of the future, or the
/// stack-trace if it panicked.
///
/// # Safety
///
/// The returned stream is expected to be run to completion.
/// For safety, the returned stream panics if dropped before
/// run to completion. It is also marked as `!Unpin` to help
/// mark the parent future which drives it also thus.
pub unsafe fn scope<'a, T: Send + 'static, R, F: FnOnce(&mut Scope<'a, T>) -> R>(
    f: F
) -> (VerifiedStream<'a, FuturesOrdered<JoinHandle<T>>>, R) {

    // We wrap the scope object in a `VerifiedStream` that
    // ensures it is driven to completion (or never
    // dropped). In either case, this implies that all
    // futures spawned in the scope have been driven to
    // completion and dropped.
    let mut scope = Scope::create();
    let op = f(&mut scope);
    (scope.futs.into(), op)
}

/// A macro that creates a scope and immediately awaits the
/// stream. The logic unwinds the stack if either the passed
/// function, or any of the futures panic. However, it still
/// awaits stream completion. This macro must be run within
/// an async block that returns a Result.
///
/// # Safety
/// To the best of our understanding, it is safe to
/// use this macro as it awaits the stream immediately and
/// catches unwinds.
#[macro_export]
macro_rules! scope_and_collect {
    ($fn: expr) => {{
        let (stream, block_output) = {
            unsafe { $crate::scope($fn) }
        };

        use async_std::prelude::StreamExt;
        let proc_outputs = StreamExt::collect::<Vec<_>>(stream).await;
        (block_output, proc_outputs)
    }}
}

/// A stream wrapper that ensures the underlying stream is
/// driven to completion before being dropped. The
/// implementation panics if the stream is incomplete.
pub struct VerifiedStream<'a, S>{
    stream: S,
    done: bool,
    _marker: PhantomData<&'a ()>,
}

impl<'a, S: 'a> From<S> for VerifiedStream<'a, S> {
    fn from(stream: S) -> VerifiedStream<'a, S> {
        VerifiedStream {
            stream,
            done: false,
            _marker: PhantomData,
        }
    }
}

impl<'a, I, S: Stream<Item=I>> VerifiedStream<'a, S> {
    fn stream(self: Pin<&mut Self>) -> Pin<&mut S> {
        // Only for projection in `poll_next`.
        unsafe { self.map_unchecked_mut(|o| &mut o.stream) }
    }

    fn done(self: Pin<&mut Self>) -> &mut bool {
        unsafe { &mut self.get_unchecked_mut().done }
    }
}

impl<'a, T> Drop for VerifiedStream<'a, T> {
    fn drop(&mut self) {
        if !self.done {
            panic!("Scoped future streams must be run to completion");
        }
    }
}

impl<'a, I, T: Stream<Item=I>> Stream for VerifiedStream<'a, T> {
    type Item = I;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context)
                 -> Poll<Option<Self::Item>> {
        let inner = self.as_mut().stream();
        let poll = inner.poll_next(cx);
        if poll.is_ready() {
            *(self.done()) = true;
        }
        poll
    }
}

#[cfg(test)]
mod tests {
    #[async_std::test]
    async fn scoped_futures() {
        let not_copy = String::from("hello world!");
        let not_copy_ref = &not_copy;

        let (stream, _) = unsafe {crate::scope(|s| {
            for _ in 0..10 {
                let proc = || async move {
                    assert_eq!(not_copy_ref, "hello world!");
                };
                s.spawn(proc());
            }
        })};

        // Uncomment this for compile error
        // std::mem::drop(not_copy);

        use futures::StreamExt;
        let count = stream.collect::<Vec<_>>().await.len();

        // Drop here is okay, as stream is already dropped.
        std::mem::drop(not_copy);

        assert_eq!(count, 10);
    }

    #[async_std::test]
    async fn scoped_await() {
        let not_copy = String::from("hello world!");
        let not_copy_ref = &not_copy;

        let (_, vals) = crate::scope_and_collect!(|s| {
            for _ in 0..10 {
                let proc = || async {
                    assert_eq!(not_copy_ref, "hello world!");
                };
                s.spawn(proc());
            }
        });

        assert_eq!(vals.len(), 10);
    }
}
