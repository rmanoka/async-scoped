# Async-scoped

Enables controlled spawning of non-`'static` futures when
using the [async-std](//github.com/async-rs/async-std) executor.

## Motivation

Present executors (such as async-std, tokio, etc.) all
support spawning `'static` futures onto a thread-pool.
However, they do not support spawning futures with lifetime
smaller than `'static`.

While the future combinators such as `for_each_concurrent`
offer concurrency, they are bundled as a single `Task`
structure by the executor, and hence are not driven
parallelly. This can be seen when benchmarking a reasonable
number (> ~1K) of I/O futures, or a few CPU heavy futures.

## Usage

The API is meant to be a minimal wrapper around efficient
executors. Currently, we only support `async_std`, but the
API easily accomodates any spawn function that just accepts
a `'static` future.

``` rust
#[async_std::test]
async fn test_scope_and_block() {
    let not_copy = String::from("hello world!");
    let not_copy_ref = &not_copy;

    let (_, vals) = async_scoped::scope_and_block(|s| {
        for _ in 0..10 {
            let proc = || async {
                assert_eq!(not_copy_ref, "hello world!");
            };
            s.spawn(proc());
        }
    });

    assert_eq!(vals.len(), 10);
}
```

## Scope API

We propose an API similar to `crossbeam::scope` to allow
controlled spawning of futures that are not `'static`. The
key function is:

``` rust
pub unsafe fn scope<'a, T: Send + 'static,
             F: FnOnce(&mut Scope<'a, T>)>(f: F)
             -> impl Stream {
    // ...
}
```

This function is used as follows:

``` rust
#[async_std::test]
async fn scoped_futures() {
    let not_copy = String::from("hello world!");
    let not_copy_ref = &not_copy;

    let (mut stream, _) = unsafe {
        async_scoped::scope(|s| {
            for _ in 0..10 {
                let proc = || async {
                    assert_eq!(not_copy_ref, "hello world!");
                };
                s.spawn(proc());
            }
        })
    };

    // Uncomment this for compile error
    // std::mem::drop(not_copy);

    use futures::StreamExt;
    let mut count = 0;
    while let Some(_) = stream.next().await {
        count += 1;
    }
    assert_eq!(count, 10);
}
```

## Cancellation

To support cancellation, `Scope` provides a
`spawn_cancellable` which wraps a future to make it
cancellable. When a `Scope` is dropped, (or if `cancel`
method is invoked), all the cancellable futures are
scheduled for cancellation. In the next poll of the
futures, they are dropped and a default value (provided
by a closure during spawn) is returned as the output of
the future.

Note that cancellation requires some reasonable
behaviour from the future and futures that do not return
control to the executor cannot be cancelled until their
next poll.

## Safety Considerations

The [`scope`][scope] API provided in this crate is
unsafe as it is possible to `forget` the stream received
from the API without driving it to completion. The only
completely (without any additional assumptions) safe API
is the [`scope_and_block`][scope_and_block] function,
which _blocks the current thread_ until all spawned
futures complete.

The [`scope_and_block`][scope_and_block] may not be
convenient in an asynchronous setting. In this case, the
[`scope_and_collect`][scope_and_collect] API may be
used. Care must be taken to ensure the returned future
is not forgotten before being driven to completion. Note
that dropping this future will lead to it being driven
to completion, while blocking the current thread to
ensure safety. However, it is unsafe to forget this
future before it is fully driven.

## Implementation

Our current implementation simply uses _unsafe_ glue to
`transmute` the lifetime, to actually spawn the futures
in the executor. The original lifetime is recorded in
the `Scope`. This allows the compiler to enforce the
necessary lifetime requirements as long as this returned
stream is not forgotten.

For soundness, we drive the stream to completion in the
[`Drop`][Drop] impl. The current thread is blocked until
the stream is fully driven.

Unfortunately, since the [`std::mem::forget`][forget]
method is allowed in safe Rust, the purely asynchronous
API here is _inherently unsafe_.

### Efficiency

Our current implementation is focussed on safety, and leaves
room for optimization. Below we list a few venues that we
hope could be further optimized.

1. The `spawn` involves an allocation (not including any
   allocation done by the executor itself). This occurs while
   transmuting the lifetime of the future, which to the best of
   our knowledge is not possible without erasing the concrete
   type of the future itself. Please see the implementation of
   `Scope::spawn` in `src/lib.rs` for more details of the
   transmute, and allocation.

1. The `CancellableFuture` wrapper also uses a synchronous
   `Mutex` and hence is not lock-free. However, the lock is
   only used to make one insertion into a `HashMap` while in
   contention.

## License

Licensed under either of [Apache License, Version
2.0](//www.apache.org/licenses/LICENSE-2.0) or [MIT
license](//opensource.org/licenses/MIT) at your
option.

Unless you explicitly state otherwise, any contribution
intentionally submitted for inclusion in this crate by you,
as defined in the Apache-2.0 license, shall be dual licensed
as above, without any additional terms or conditions.
