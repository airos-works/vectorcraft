# Haiku Vulkan support

`wgpu`, `wgpu-core` and `wgpu-core-deps-windows-linux-android` 30.0.1 are copied from crates.io,
with their upstream MIT/Apache-2.0 licenses (as in the other Craft apps' air/OS ports). The only
changes add `target_os = "haiku"` alongside FreeBSD in platform dependency and backend cfg lists
(`Cargo.toml`, `Cargo.toml.orig`, `build.rs`). This makes their existing Vulkan backend available
on air/OS. There is no new Vulkan implementation or window surface; the native host
(`apps/vectorcraft-haiku`) renders offscreen and presents the finished frame.
