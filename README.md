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
async fn test_scope_and_collect() {
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

    let mut stream = unsafe { crate::scope(|s| {
        for _ in 0..10 {
            let proc = async || {
                assert_eq!(not_copy_ref, "hello world!");
            };
            s.spawn(proc());
        }
    }) };

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

## Safety Considerations

The `scope` API provided in this crate is unsafe (see the
first point below). Here, we list the key reasons for
unsafety, towards identifying a safe usage. The safe usage
are facilitated by `scope_and_collect` and
`scope_and_iterate` macros available in this crate.

1. Since safe Rust allows `forget`-ting the returned
   `Stream`, the onus of actually driving it to completion
   is on the user.

2. The spawned future _must be_ dropped immediately after
   completion as it may contain references that are soon to
   expire. This is the behaviour of most executors (incl.
   `async-std`, `tokio`), and is crucial here.

3. The `poll` of the `Task` containing the parent `Stream`
   must not move or drop `self` before completion of all
   spawned futures

4. The above should hold even if some other futures in the
   `Task` panic. To our understanding, `async_std` assumes a
   "panic == abort" model, and this somewhat simplifies our
   concerns here. In particular, we _do not wrap_ the
   spawned futures with `catch_unwind`, and `resume_unwind` in
   this implementation.

## Implementation

Our current implementation simply uses `unsafe` glue to
actually spawn the futures. Then, it records the lifetime of
the futures in the returned `Stream` object.

Currently, for soundness, we simply panic! if the stream is
dropped before it is fully driven. Another option (not
implemented here), may be to drive the stream using a
current-thread executor inside the `Drop` impl. Would be
great to hear any thoughts on the problem, and the safety of
this implementation.

Unfortunately, since the `std::mem::forget` method is safe,
the API here is _inherently unsafe_.

### Efficiency

Our implementation minimizes allocations to the best of our
efforts. Unfortunately, this still amounts to one allocation
per `spawn` call (not including any allocation done by the
executor itself). This occurs while transmuting the lifetime
of the future, which to the best of our knowledge is not
possible without erasing the concrete type of the future
itself. Please see the implementation of `Scope::spawn` in
`src/lib.rs` for more details of the transmute, and
allocation.
