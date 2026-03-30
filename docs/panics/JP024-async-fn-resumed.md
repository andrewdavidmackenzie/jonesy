---
layout: default
title: "JP024: Async Function Resumed After Completion"
---

# JP024: Async Function Resumed After Completion

**Severity**: Error
**Category**: Runtime Errors

## Description

An async function's future was polled after it already returned `Poll::Ready`. The Rust compiler generates a poll-after-completion check in every async function's state machine that panics with `panic_const_async_fn_resumed` if this occurs.

## Example

```rust
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

async fn my_task() {
    // ... async work ...
}

// Incorrect executor that polls after completion:
fn bad_executor(future: Pin<&mut impl Future<Output = ()>>, cx: &mut Context<'_>) {
    let _ = future.poll(cx);  // Returns Ready
    let _ = future.poll(cx);  // JP024: panics!
}
```

## Why It Happens

Every `async fn` compiles to a state machine with states like Unresumed, Running, and Complete. The compiler inserts a check: if the state machine is polled after reaching Complete, it panics with `panic_const_async_fn_resumed`.

Common causes:
- Buggy custom `Future` executor or runtime implementation
- Manually polling a future after it returned `Poll::Ready`
- Incorrect `select!` or `join!` logic that re-polls completed futures

Note: standard async runtimes (tokio, async-std, smol) handle this correctly. This panic typically only occurs with custom executor code or unsafe future manipulation.

## How to Avoid

### Track completion state

```rust
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

fn poll_once(future: Pin<&mut impl Future<Output = ()>>, cx: &mut Context<'_>) -> bool {
    matches!(future.poll(cx), Poll::Ready(()))
}
```

### Use FusedFuture

```rust
use futures::future::FusedFuture;

async fn safe_select(a: impl FusedFuture, b: impl FusedFuture) {
    // FusedFuture tracks whether it's terminated
    // and returns Poll::Pending after completion instead of panicking
}
```

### Use established async runtimes

```rust
// tokio, async-std, and smol all handle poll-after-completion correctly
#[tokio::main]
async fn main() {
    my_task().await;  // Safe: runtime manages polling
}
```

## Jonesy Output

```text
 --> src/lib.rs:5:1 [JP024: async function polled after completion]
     = help: Ensure futures are not polled after returning Poll::Ready; check executor logic
```

## Related

- [JP015 - Panic in Drop](/jonesy/panics/JP015-panic-in-drop): Other compiler-generated panic paths
- [JP016 - Cannot Unwind](/jonesy/panics/JP016-cannot-unwind): Panics in restricted contexts
