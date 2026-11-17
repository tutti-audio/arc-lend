#![doc = include_str!("../README.md")]
//! Sharing an [`Arc`] with threads that must never free.
//!
//! A [`Publisher`] holds an `Arc` that one thread can replace while others
//! read it through [`Reader`]s. In that respect it is similar to [`ArcSwap`],
//! or to `RwLock<Arc<T>>` without the lock. The difference is in who lets go
//! of the old value: here it is always the writer, never a reader. It is
//! meant for threads that run against a deadline, such as an audio callback, a
//! game's fixed tick or a control loop.
//!
//! ## Why it exists
//!
//! Picture an audio callback. It has about a millisecond to fill each buffer,
//! and while it runs, the user keeps turning knobs, so the filter settings it
//! reads are replaced every so often. The settings are too big for an atomic,
//! so they live behind an `Arc` that the writer swaps out.
//!
//! That works until the moment a swap lands while the callback is still
//! reading the old settings. Then the callback holds the last reference, and
//! when it lets go, the old settings are freed right there, inside the
//! millisecond.
//!
//! We wanted reads that are **wait-free**, **allocation-free** and
//! **free-free** (a reader never runs a destructor), whatever the timing, and a
//! writer that **never waits on a reader**.
//!
//! ## The idea: lend, don't share
//!
//! With an `Arc`, every reader is a co-owner, and whichever owner is last
//! cleans up. Here a single owner, the cell itself, *lends* the
//! value to readers instead. A reader marks what it has borrowed while it reads,
//! and clears the mark when it is done. Before freeing an old value, the writer
//! checks the marks. Anything still on loan is kept and freed on a later pass.
//!
//! So a read is wait-free, allocation-free and free-free, and all the cleanup
//! happens on the writer's thread. The price is paid on the writer's side too: each reading thread
//! registers once, ahead of time, and the writer's work grows with the number
//! of registered readers.
//!
//! ## Using it
//!
//! 1. Wrap the value in a [`Publisher`], which the writing side keeps.
//! 2. Before each reading thread starts, register a [`Reader`] for it with
//!    [`reader`]. This is the one step that may allocate, so it belongs off
//!    the deadline.
//! 3. On the reading thread, call [`lend`] once per iteration. The [`Loan`]
//!    it returns derefs to the value, and dropping it never frees. A reader
//!    can do nothing else: it holds no way to publish or reclaim.
//! 4. On the writing thread, [`publish`] each new version. Old versions are
//!    freed there, once no reader still has them.
//!
//! ```rust
//! use arc_lend::Publisher;
//!
//! struct Settings { gains: Vec<f32> }
//!
//! // 1. The cell, and the writer's handle to it.
//! let publisher = Publisher::new(Settings { gains: vec![1.0; 4] });
//!
//! // 2. Register the reader on the writer's side, then move it to its thread.
//! let mut reader = publisher.reader();
//! let reader_thread = std::thread::spawn(move || {
//!     for _iteration in 0..1000 {
//!         // 3. One lend per iteration: wait-free, allocation-free, free-free.
//!         let settings = reader.lend();
//!         let total: f32 = settings.gains.iter().sum();
//!     }
//! });
//!
//! // 4. The writer publishes new versions, and frees old ones here.
//! for n in 1..100 {
//!     publisher.publish(Settings { gains: vec![0.5; 4 + n % 3] });
//! }
//! reader_thread.join().unwrap();
//! ```
//!
//! The writer can also read the value back with [`current`], which is handy
//! for deriving the next version from the last. Several control-side owners
//! can share one cell by cloning the `Publisher`.
//!
//! ## Is it the right fit?
//!
//! Probably not, if none of your threads has a deadline. [`ArcSwap`] is then
//! the more general tool: any thread can read without registering first, a
//! reader can keep what it reads as its own `Arc`, and it offers more ways to
//! update.
//! This crate gives those up so that its reads can be free-free, and earns its
//! keep when a stray free on the reading thread would actually hurt.
//!
//! It also asks a little of you: a loan should last one iteration, not be kept
//! around, and a reader paused at an unlucky moment can make the writer hold on
//! to old versions for longer. The [guide] goes through these.
//!
//! ## Further reading
//!
//! - [Guide][guide]: how to use it well, and what to watch for.
//! - [Benchmarks][benchmarks]: what it costs, measured against the
//!   alternatives.
//! - [Design notes][design]: how it works, and why we believe it is safe.
//!
//! [`Arc`]: https://doc.rust-lang.org/std/sync/struct.Arc.html
//! [`ArcSwap`]: https://docs.rs/arc-swap
//! [`Publisher`]: crate::Publisher
//! [`Reader`]: crate::Reader
//! [`Loan`]: crate::Loan
//! [`reader`]: crate::Publisher::reader
//! [`publish`]: crate::Publisher::publish
//! [`current`]: crate::Publisher::current
//! [`lend`]: crate::Reader::lend
//! [guide]: crate::docs::guide
//! [benchmarks]: https://github.com/tutti-audio/arc-lend/blob/main/BENCHMARKS.md
//! [design]: crate::docs::design
#![deny(clippy::undocumented_unsafe_blocks, clippy::missing_safety_doc)]

mod current;
mod publisher;
mod reader;
mod shared;
mod slots;
mod sync;

pub mod docs;

pub use publisher::Publisher;
pub use reader::{Loan, Reader};

// Under `--cfg loom` the atomics are loom's, which panic outside a
// `loom::model`, so these ordinary tests are compiled out; the loom model is
// `tests/loom.rs`.
#[cfg(all(test, not(loom)))]
mod tests;
