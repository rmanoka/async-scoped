//! Enables controlled spawning of non-`'static` futures
//! when using the [async-std][async_std] executor. Note
//! that this idea is similar to `crossbeam::scope`, and
//! `rayon::scope` but asynchronous.
//!
//! ## Motivation
//!
//! Executors like async_std, tokio, etc. support spawning
//! `'static` futures onto a thread-pool. However, it is
//! often useful to spawn futures that may not be `'static`.
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
//!     let (foo, outputs) = async_scoped::scope_and_block(|s| {
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
//! should ensure that the returned future is not forgetten
//! before being driven to completion.
//!
//! ## Safety Considerations
//!
//! The [`scope`][scope] API provided in this crate is
//! unsafe as it is possible to `forget` the stream received
//! from the API without driving it to completion. The only
//! completely (without any additional assumptions) safe API
//! is the [`scope_and_block`][scope_and_block] function,
//! which _blocks the current thread_ until all spawned
//! futures complete.
//!
//! The [`scope_and_block`][scope_and_block] may not be
//! convenient in an asynchronous setting. In this case, the
//! [`scope_and_collect`][scope_and_collect] API may be
//! used. Care must be taken to ensure the returned future
//! is not forgotten before being driven to completion. Note
//! that dropping this future will lead to it being driven
//! to completion, while blocking the current thread to
//! ensure safety. However, it is unsafe to forget this
//! future before it is fully driven.
//!
//! ## Implementation
//!
//! Our current implementation simply uses _unsafe_ glue to
//! `transmute` the lifetime, to actually spawn the futures
//! in the executor. The original lifetime is recorded in
//! the `Scope` and hence the returned
//! [stream][VerifiedStream] object. This allows the
//! compiler to enforce the necessary lifetime requirements
//! as long as this returned stream is not forgotten.
//!
//! For soundness, we drive the stream to completion in the
//! [`Drop`][Drop] impl. The current thread is blocked until
//! the stream is fully driven.
//!
//! Unfortunately, since the [`std::mem::forget`][forget]
//! method is allowed in safe Rust, the purely asynchronous
//! API here is _inherently unsafe_.
//!
//! [async-std]: async_std
//! [poll]: std::futures::Future::poll
//! [Task]: std::task::Task
//! [forget]: std::mem::forget
//! [Stream]: futures::Stream
//! [for_each_concurrent]: futures::StreamExt::for_each_concurrent
use futures::{Future, FutureExt};
use futures::future::BoxFuture;
use futures::stream::FuturesOrdered;
use async_std::task::JoinHandle;
use async_std::sync::RwLock;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::task::Waker;

mod verified_stream;
pub use verified_stream::VerifiedStream;

mod cancellable_future;
pub use cancellable_future::CancellableFuture;

/// A scope to allow controlled spawning of non 'static
/// futures.
pub struct Scope<'a, T: Send + 'static> {
    lock: Arc<RwLock<bool>>,
    read_wakers: Arc<Mutex<Vec<Waker>>>,
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
            lock: Arc::new(RwLock::new(true)),
            read_wakers: Arc::new(Mutex::new(Vec::new())),
            futs: FuturesOrdered::new(),
            _marker: PhantomData,
        }
    }

    /// Spawn a future with `async_std::task::spawn`. The
    /// future is expected to be driven to completion before
    /// 'a expires.
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

    /// Spawn a cancellable future with `async_std::task::spawn`
    ///
    /// The future is may be cancelled if the
    /// `VerifiedStream` returned by the calling `Scope` is
    /// dropped pre-maturely.
    ///
    /// # Safety
    ///
    /// This function is safe as it is unsafe to create a
    /// `Scope` object. The creator of the object is
    /// expected to enforce the lifetime guarantees.
    pub fn spawn_cancellable<F: Future<Output=T> + Send + 'a,
                             Fu: FnOnce() -> T + Send + 'a>(&mut self, f: F, cancellation: Fu) {
        self.spawn(CancellableFuture::new(self.lock.clone(), self.read_wakers.clone(), f, cancellation))
    }

    /// Convert `Scope` into a `Stream` of spawned future outputs.
    ///
    /// This function is useful when spawning in an async
    /// context. This allows, for instance, to apply
    /// back-pressure.
    pub fn into_stream(self) -> VerifiedStream<'a, FuturesOrdered<JoinHandle<T>>> {
        let len = self.futs.len();
        VerifiedStream::new(self.futs, len, self.lock, self.read_wakers)
    }
}

/// Creates a `Scope` to spawn non-'static futures. The
/// function is called with a block which takes an `&mut
/// Scope`. The `spawn` method on this arg. can be used to
/// spawn "local" futures.
///
/// # Returns
///
/// The function returns a tuple of `VerifiedStream`, and
/// the return value of the block passed to it. The returned
/// stream and is expected to be driven completely before
/// being forgotten. Dropping this stream causes the stream
/// to be driven _while blocking the current thread_. The
/// values returned from the stream are the output of the
/// futures spawned.
///
/// # Safety
///
/// The returned stream is expected to be run to completion
/// before being forgotten. Dropping it is okay, but blocks
/// the current thread until all spawned futures complete.
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
    (scope.into_stream(), op)
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
pub fn scope_and_block<
        'a,
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
/// the output of the block.
///
/// # Safety
///
/// This function is _not completely safe_: please see
/// [tests.rs][tests-src] for a cancellation test-case that
/// can lead to invalid memory access.
///
/// The caller must ensure that the lifetime 'a is valid
/// until the returned future is fully driven. Dropping it
/// is okay, but blocks the current thread until all spawned
/// futures complete.
///
/// [tests-src]: https://github.com/rmanoka/async-scoped/blob/master/src/tests.rs
pub async unsafe fn scope_and_collect<
        'a,
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
/// This function is _not completely safe_: please see
/// [tests.rs][tests-src] for a cancellation test-case that
/// can lead to invalid memory access.
///
/// The caller must ensure that the lifetime 'a is valid
/// until the returned future is fully driven. Dropping it
/// is okay, but blocks the current thread until all spawned
/// futures complete.
///
/// [tests-src]: https://github.com/rmanoka/async-scoped/blob/master/src/tests.rs
pub async unsafe fn scope_and_iterate<
        'a,
    T: Send + 'static, R,
    F: FnOnce(&mut Scope<'a, T>) -> R,
    G: FnMut(T) -> H,
    H: Future<Output=()>
        >(f: F, g: G) -> R {

    let (stream, block_output) = scope(f);
    futures::StreamExt::for_each(stream, g).await;
    block_output

}

#[cfg(test)]
mod tests;
