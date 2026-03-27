---
layout: default
title: "JP011: Debug Assert Failed (Removed)"
redirect_to: /panics/JP010-assert-failed
---

# JP011: Debug Assert Failed (Removed)

This error code has been merged into [JP010 - Assert Failed](/panics/JP010-assert-failed).

Both `assert!()` and `debug_assert!()` compile to the same `assert_failed` function, so they cannot be distinguished at the binary level. All assertion failures are now reported as JP010.
