# [XS] Bindings default to zero max delay

## Goal

A subscription built with no arguments skips superseded groups at once in
every language, as Rust, JS, Python, and the released bindings do. `null`
(Rust `None`) keeps meaning "no subscriber limit", distinct from zero, and a
reader asks for it explicitly.

## Plan

Decided 2026-10-27 (maintainer), from an external release-to-main audit
that found the regression in Dart; it reaches every binding that exposes
the generated record directly.

`MoqSubscription.max_delay` is `Option<Duration>` with no UniFFI default,
because UniFFI cannot spell a duration literal (`uniffi_meta` refuses an
integer default on a `Duration` field), so a bare generated record carries
`None`, which moq-net reads as a `Duration::MAX` budget, while its own
`Subscription::default()` is `Some(Duration::ZERO)`.

Python already wrapped the record with a zero default. Dart (a `Subscription`
subclass), Swift and Kotlin (an initializer or factory without `maxDelay`),
and Go (`MaxDelay time.Duration` with a `NoMaxDelay` sentinel) now default
to zero too. The other `max_delay` records (fetch options, decoder outputs)
match their moq-net and moq-mux defaults.

Remaining: the generated-only C++ (`cpp/moq`) and C (`c/moq`) packages.
`moq::Subscription{}` and `moq_subscription_default()` leave `max_delay`
unset, so they still mean no subscriber limit. Those packages hand-write
nothing by design, so the fix needs a maintainer call: document that
`max_delay` must be set there, teach the `kixelated/uniffi-bindgen-cpp`
fork a per-field default, or land a duration default literal in UniFFI
itself, which would also let the other wrappers drop their hand-written
defaults.

Public API: breaking for Dart and Go, whose `Subscription` changes type or
default; additive for Swift and Kotlin. Wire: none.
