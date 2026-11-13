# arc-lend

[![crates.io](https://img.shields.io/crates/v/arc-lend.svg)](https://crates.io/crates/arc-lend)
[![docs.rs](https://img.shields.io/docsrs/arc-lend)](https://docs.rs/arc-lend)
[![CI](https://github.com/tutti-audio/arc-lend/actions/workflows/ci.yml/badge.svg)](https://github.com/tutti-audio/arc-lend/actions/workflows/ci.yml)

arc-lend is a shared-value cell built for threads that run against a deadline: one thread replaces the value while others read it without waiting or freeing memory.

Such a thread might be an audio callback, a game's fixed update or a control loop. The writer keeps ownership of old values and frees them later, so readers only borrow.

arc-lend was built for [tutti](https://github.com/tutti-audio/tutti), a real-time audio engine for Rust, but does not depend on it.

The [documentation](https://docs.rs/arc-lend) shows how to use it, and its [design](https://docs.rs/arc-lend/latest/arc_lend/docs/design/index.html) page explains how it works. [BENCHMARKS.md](https://github.com/tutti-audio/arc-lend/blob/main/BENCHMARKS.md) compares it with arc-swap, basedrop, left-right and others.

## Supported Rust

arc-lend builds on the Rust version named by `rust-version` in its `Cargo.toml`. Raising it is a minor release.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/tutti-audio/arc-lend/blob/main/LICENSE-APACHE) or [MIT license](https://github.com/tutti-audio/arc-lend/blob/main/LICENSE-MIT) at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in arc-lend by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
