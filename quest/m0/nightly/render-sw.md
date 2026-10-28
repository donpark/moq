# [S] Software rendering runs every group

## Goal

Nightly's Tests (rs render-sw) passes all three of its groups under software
Vulkan, so the export, scene, and studio checks after the first one run again.

## Plan

Nightly [40506938673](https://github.com/moq-dev/moq/actions/runs/40506938673) (2026-10-27, at 22808bbb2f) fails the first group on
`the_output_follows_the_device` (`rs/moq-video/src/render/renderer.rs:528`,
from [moq-dev/moq#6806](https://github.com/moq-dev/moq/pull/6806)), which
asserts "the adapter imports DMA-BUFs". Software Vulkan cannot, so the
assertion belongs only where the device can: make the test check what the
device reports it supports, or mark the DMA-BUF half with
`#[ignore = "requires ..."]` and let `just rs gpu` select it. The recipe's
later groups must not depend on the first passing (see this epic's
continue-past-failure work).

Public API: none. Wire: none.
