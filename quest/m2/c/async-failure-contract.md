# [XS] Waiting: generated C async calls keep their failure contract

## Goal

Every async call in the generated C API has a callback contract a C caller can rely on for freeing `user_data`: the call returns a task, never NULL, and its callback runs exactly once, with the result or, when the task was freed first, with `cancelled` set.

Waiting on [kixelated/rust2#169](https://github.com/kixelated/rust2/pull/169), which makes cancellation a terminal result in rust2's C generator; delete this quest once it merges. c/moq adopts the new callback shape (`bool cancelled` after `user_data`) when [Switch to rust2's UniFFI](/quest/m2/uniffi-fork.md) moves the C generator pin, along with the CamelCase names, and `c/moq/test/probe.c`'s callbacks handle `cancelled`.

## Plan

From the 2026-10-22 security audit, as rust2's PR implements it:

- No generated async call fails before returning a task: arguments convert infallibly and misuse aborts, so the immediate-failure rule reduces to "returns a task, never NULL", stated once in the header's conventions and on each callback typedef.
- Freeing a pending task dispatches its callback once with `cancelled` set, on the dispatcher like a result, so it never nests inside the freeing call. The result and the cancel share one atomic claim, and `task_free` no longer blocks on a callback running elsewhere.
- A Rust panic aborts the process, as it does for every sync call under the expected style, so no callback is lost while the process lives. C++ mutexes do not poison. A dispatcher that refuses work or has shut down still abandons the call.

Public API: every generated async callback gains `bool cancelled`, and a cancelled task calls back once. Wire: none.
