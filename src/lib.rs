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
//! should ensure that the returned future _is not
//! forgetten_ before being driven to completion.
//!
//! ## Cancellation
//!
//! To support cancellation, `Scope` provides a
//! `spawn_cancellable` which wraps a future to make it
//! cancellable. When a `Scope` is dropped, (or if `cancel`
//! method is invoked), all the cancellable futures are
//! scheduled for cancellation. In the next poll of the
//! futures, they are dropped and a default value (provided
//! by a closure during spawn) is returned as the output of
//! the future.
//!
//! Note that cancellation requires some reasonable
//! behaviour from the future and futures that do not return
//! control to the executor cannot be cancelled until their
//! next poll.
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
//! the `Scope`. This allows the compiler to enforce the
//! necessary lifetime requirements as long as this returned
//! stream is not forgotten.
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
mod cancellable_future;
pub(crate) use cancellable_future::CancellableFuture;

mod scoped;
pub use scoped::Scope;

mod usage;
pub use usage::{scope, scope_and_block, scope_and_collect};

#[cfg(test)]
mod tests;
