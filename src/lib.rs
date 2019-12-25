//! Enables controlled spawning of non-`'static` futures
//! when using the [async-std][async_std] executor. Note
//! that this idea is similar to used in `crossbeam::scope`,
//! and `rayon::scope` but asynchronous.
//!
//! ## Motivation
//!
//! Executors like async_std, tokio, etc. support spawning
//! `'static` futures onto a thread-pool. However, it is
//! often useful to spawn a stream of futures that may not
//! all be `'static`.
//!
//! While the future combinators such as
//! [`for_each_concurrent`][for_each_concurrent] offer
//! concurrency, they are bundled as a single [`Task`][Task]
//! structure by the executor, and hence are not driven
//! parallelly.
//!
//! ## Scope API
//!
//! We propose an API similar to
//! [`crossbeam::scope`](crossbeam::scope) to allow
//! controlled spawning of futures that are not `'static`.
//! The key function is:
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
//!     let (foo, outputs) = crate::scope_and_block(|s| {
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
//! The `scope_and_block` function above blocks the current
//! thread in order to guarantee safety. We also provide an
//! unsafe `scope_and_collect`, which is asynchronous, and
//! does not block the current thread. However, the user
//! should ensure that the future that calls this function
//! is not cancelled before all the spawned futures are
//! driven to completion.
//!
//! ## Safety Considerations
//!
//! The [`scope`][scope] API provided in this crate is
//! inherently unsafe. Here, we list the key reasons for
//! unsafety, towards identifying a safe usage (facilitated
//! _only_ by [`scope_and_block`][scope_and_block] function).
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
//!    async function that uses [`scope`][scope] is
//!    [`!Unpin`][!Unpin].
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
//! Currently, for soundness, we panic! if the stream is
//! dropped before it is fully driven. Another option (not
//! implemented here), may be to drive the stream using a
//! current-thread executor inside the [`Drop`][Drop] impl.
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
    _marker: PhantomData<fn(&'a  ())>
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

/// A function that creates a scope and immediately awaits,
/// _blocking the current thread_ for spawned futures to
/// complete. The outputs of the futures are collected as a
/// `Vec` and returned along with the output of the block.
///
/// # Safety
///
/// This function is safe to the best of our understanding
/// as it blocks the current thread until the stream is
/// driven to completion, implying that all the spawned
/// futures have completed too. However, care must be taken
/// to ensure a recursive usage of this function doesn't
/// lead to deadlocks.
///
/// When scope is used recursively, you may also use the
/// unsafe `scope_and_*` functions as long as this function
/// is used at the top level. In this case, either the
/// recursively spawned should have the same lifetime as the
/// top-level scope, or there should not be any spurious
/// future cancellations within the top level scope.
pub fn scope_and_block
    <'a,
     T: Send + 'static, R,
     F: FnOnce(&mut Scope<'a, T>) -> R
     >(f: F) -> (R, Vec<T>) {

        let (mut stream, block_output) = {
            unsafe { scope(f) }
        };

        let mut proc_outputs = Vec::with_capacity(stream.len);
        async_std::task::block_on(async {
            while let Some(item) = futures::StreamExt::next(&mut stream).await {
                proc_outputs.push(item);
            }
        });

        (block_output, proc_outputs)
}

/// An asynchronous function that creates a scope and
/// immediately awaits the stream. The outputs of the
/// futures are collected as a `Vec` and returned along with
/// the output of the block. This macro _must be invoked_
/// within an async block.
///
/// # Safety
///
/// This macro is _not completely safe_: please see
/// https://www.reddit.com/r/rust/comments/ee3vsu/asyncscoped_spawn_non_static_futures_with_asyncstd/fbpis3c?utm_source=share&utm_medium=web2x
/// The caller must ensure the enclosing async block (and
/// it's stack) does not collapse before the macro completes
/// driving the stream. However, unless the enclosing future
/// gets forgotten, the implementation will still panic when
/// the returned future is dropped without being fully
/// driven.
pub async unsafe fn scope_and_collect
    <'a,
     T: Send + 'static, R,
     F: FnOnce(&mut Scope<'a, T>) -> R
     >(f: F) -> (R, Vec<T>) {

        let (mut stream, block_output) = scope(f);

        let mut proc_outputs = Vec::with_capacity(stream.len);
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            proc_outputs.push(item);
        }
        (block_output, proc_outputs)
}

/// An asynchronous function that creates a scope and
/// immediately awaits the stream, and sends it through an
/// FnMut (using `futures::StreamExt::for_each`). It takes
/// two args, the first that spawns the futures, and the
/// second is the function to call on the stream. This macro
/// _must be invoked_ within an async block.
///
/// # Safety
///
/// This macro is _not completely safe_: please see
/// https://www.reddit.com/r/rust/comments/ee3vsu/asyncscoped_spawn_non_static_futures_with_asyncstd/fbpis3c?utm_source=share&utm_medium=web2x
/// The caller must ensure the enclosing async block (and
/// it's stack) does not collapse before the macro completes
/// driving the stream. However, unless the enclosing future
/// gets forgotten, the implementation will still panic when
/// the returned future is dropped without being fully
/// driven.
pub async unsafe fn scope_and_iterate
    <'a,
     T: Send + 'static, R,
     F: FnOnce(&mut Scope<'a, T>) -> R,
     G: FnMut(T) -> H,
     H: Future<Output=()>
     >(f: F, g: G) -> R {

        let (stream, block_output) = scope(f);
        futures::StreamExt::for_each(stream, g).await;
        block_output

}

/// DEPRECATED: please use the function variant.
///
/// A macro that creates a scope and immediately awaits,
/// _blocking the current thread_ for spawned futures to
/// complete. The outputs of the futures are collected as a
/// `Vec` and returned along with the output of the block.
///
/// # Safety
///
/// This macro is safe to the best of our understanding.
#[macro_export]
macro_rules! scope_and_block {
    ($fn: expr) => {{
        let (mut stream, block_output) = {
            unsafe { $crate::scope($fn) }
        };

        let mut proc_outputs = Vec::with_capacity(stream.len);
        async_std::task::block_on(async {
            while let Some(item) = futures::StreamExt::next(&mut stream).await {
                proc_outputs.push(item);
            }
        });

        (block_output, proc_outputs)
    }}
}

/// DEPRECATED: please use the function variant.
///
/// A macro that creates a scope and immediately awaits the
/// stream. The outputs of the futures are collected as a
/// `Vec` and returned along with the output of the block.
/// This macro _must be invoked_ within an async block.
///
/// # Safety This macro is _not completely safe_: please see
/// https://www.reddit.com/r/rust/comments/ee3vsu/asyncscoped_spawn_non_static_futures_with_asyncstd/fbpis3c?utm_source=share&utm_medium=web2x
/// The caller must ensure the enclosing async block (and
/// it's stack) does not collapse before the macro completes
/// driving the stream. However, unless the enclosing future
/// gets forgotten, the implementation will still panic when
/// the returned future is dropped without being fully
/// driven.
#[macro_export]
macro_rules! unsafe_scope_and_collect {
    ($fn: expr) => {{
        let (mut stream, block_output) = {
            unsafe { $crate::scope($fn) }
        };
        let mut proc_outputs = Vec::with_capacity(stream.len);
        while let Some(item) = futures::StreamExt::next(&mut stream).await {
            proc_outputs.push(item);
        }
        (block_output, proc_outputs)
    }}
}

/// DEPRECATED: please use the function variant.
///
/// A macro that creates a scope and immediately awaits the
/// stream, and sends it through an FnMut. It takes two
/// args, the first that spawns the futures, and the second
/// is the function to call on the stream. This macro _must be
/// invoked_ within an async block.
///
/// # Safety This macro is _not completely safe_: please see
/// https://www.reddit.com/r/rust/comments/ee3vsu/asyncscoped_spawn_non_static_futures_with_asyncstd/fbpis3c?utm_source=share&utm_medium=web2x
/// The caller must ensure the enclosing async block (and
/// it's stack) does not collapse before the macro completes
/// driving the stream. However, unless the enclosing future
/// gets forgotten, the implementation will still panic when
/// the returned future is dropped without being fully
/// driven.
#[macro_export]
macro_rules! unsafe_scope_and_iterate {
    ($fn: expr, $iter_fn: expr) => {{
        let (stream, block_output) = {
            unsafe { $crate::scope($fn) }
        };
        futures::StreamExt::for_each(stream, $iter_fn).await;
        block_output
    }}
}

/// A stream wrapper that ensures the underlying stream is
/// driven to completion before being dropped. The
/// implementation panics if the stream is incomplete.
pub struct VerifiedStream<'a, S>{
    stream: S,
    pub done: bool,
    pub len: usize,
    _marker: PhantomData<&'a ()>,
}

impl<'a, I: std::future::Future>
From<FuturesOrdered<I>> for VerifiedStream<'a, FuturesOrdered<I>> {
    fn from(stream: FuturesOrdered<I>)
            -> VerifiedStream<'a, FuturesOrdered<I>> {
        VerifiedStream {
            len: stream.len(),
            done: false,
            _marker: PhantomData,
            stream,
        }
    }
}

impl<'a, I, S: Stream<Item=I>> VerifiedStream<'a, S> {
    fn stream(self: Pin<&mut Self>) -> Pin<&mut S> {
        // Only for projection in `poll_next`.
        unsafe { self.map_unchecked_mut(|o| &mut o.stream) }
    }

    fn done(self: Pin<&mut Self>) -> &mut bool {
        // Only for projection in `poll_next`.
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
    async fn scope() {
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

        // Drop here is okay, as stream has been consumed.
        std::mem::drop(not_copy);
        assert_eq!(count, 10);
    }

    #[async_std::test]
    async fn scope_and_collect() {
        let not_copy = String::from("hello world!");
        let not_copy_ref = &not_copy;

        let (_, vals) = unsafe { crate::scope_and_collect(|s| {
            for _ in 0..10 {
                let proc = || async {
                    assert_eq!(not_copy_ref, "hello world!");
                };
                s.spawn(proc());
            }
        }) }.await;

        assert_eq!(vals.len(), 10);
    }

    #[async_std::test]
    async fn scope_and_iterate() {
        let not_copy = String::from("hello world!");
        let not_copy_ref = &not_copy;
        let mut count = 0;

        unsafe { crate::scope_and_iterate(|s| {
            for _ in 0..10 {
                let proc = || async {
                    assert_eq!(not_copy_ref, "hello world!");
                };
                s.spawn(proc());
            }
        }, |_| {
            count += 1;
            futures::future::ready(())
        }) }.await;

        assert_eq!(count, 10);
    }

    #[async_std::test]
    async fn scope_and_block() {
        let not_copy = String::from("hello world!");
        let not_copy_ref = &not_copy;

        let ((), vals) = crate::scope_and_block(|s| {
            for _ in 0..10 {
                let proc = || async {
                    assert_eq!(not_copy_ref, "hello world!");
                };
                s.spawn(proc());
            }
        });

        assert_eq!(vals.len(), 10);
    }

    /// This is a simplified version of the soundness bug
    /// pointed out on reddit. Here, we test that it does
    /// not happen when using the `scope_and_block`
    /// function. Using `scope_and_collect` here should
    /// trigger a panic.
    #[async_std::test]
    async fn cancellation_soundness() {
        use async_std::future;
        use std::time::Duration;

        async fn inner() {
            let mut shared = true;
            let shared_ref = &mut shared;

            let mut fut = Box::pin(
                // Change next line to below for panic.
                // unsafe { crate::scope_and_collect(|scope| {
                async { crate::scope_and_block(|scope| {
                    scope.spawn(async {
                        assert!(future::timeout(
                            Duration::from_millis(100),
                            future::pending::<()>(),
                        ).await.is_err());
                        assert!(*shared_ref);
                    });
                })}
            );
            #[allow(unused_must_use)]
            let _ = future::timeout(Duration::from_millis(10), &mut fut).await;
            std::mem::forget(fut);
        }

        inner().await;
        assert!(future::timeout(Duration::from_millis(100),
                        future::pending::<()>()).await.is_err());

    }

    // #[async_std::test]
    // async fn async_deadlock() {
    //     use std::future::Future;
    //     use futures::FutureExt;
    //     femme::start(log::LevelFilter::Trace).unwrap();

    //     fn nth(n: usize) -> impl Future<Output=usize> + Send {
    //         eprintln!("@nth, n={}", n);
    //         async move {
    //             eprintln!("@block_on, n={}", n);
    //             async_std::task::block_on(async move {

    //                 if n == 0 {
    //                     0
    //                 } else {
    //                     eprintln!("@spawn, n={}", n);
    //                     let fut = async_std::task::spawn(nth(n-1)).boxed();
    //                     fut.await + 1
    //                 }

    //             })
    //         }
    //     }
    //     let input = 5;
    //     assert_eq!(nth(input).await, input);
    // }

    // Mutability test: should fail to compile.
    // TODO: use compiletest_rs
    // #[async_std::test]
    // async fn mutating_scope() {
    //     let mut not_copy = String::from("hello world!");
    //     let not_copy_ref = &mut not_copy;
    //     let mut count = 0;

    //     crate::scope_and_iterate!(|s| {
    //         for _ in 0..10 {
    //             let proc = || async {
    //                 not_copy_ref.push('.');
    //             };
    //             s.spawn(proc()); //~ ERROR
    //         }
    //     }, |_| {
    //         count += 1;
    //         futures::future::ready(())
    //     });

    //     assert_eq!(count, 10);
    // }

    // StreamExt::collect of async_std does not preserve Send trait.
    // Uncomment this for test compilation error (add unstable in Cargo.toml)
    // #[async_std::test]
    // async fn send() {
    //     fn test_send_trait<T: Send>(_: &T) {}

    //     let stream = futures::stream::pending::<()>();
    //     test_send_trait(&stream);

    //     use async_std::prelude::StreamExt;
    //     let fut = stream.collect::<Vec<_>>();
    //     test_send_trait(&fut);
    // }
}
