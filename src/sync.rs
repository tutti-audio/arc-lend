//! Atomics, fences and the mutex: `std`'s in a normal build, `loom`'s under
//! `--cfg loom`, so the model in `tests/loom.rs` checks *this* code rather
//! than a replica of it.

#[cfg(loom)]
pub(crate) use loom::{
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering, fence},
    sync::{Mutex, MutexGuard},
};
#[cfg(not(loom))]
pub(crate) use std::{
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering, fence},
    sync::{Mutex, MutexGuard},
};

/// The publish counter's atomic: 64 bits where the target has them.
#[cfg(any(loom, target_has_atomic = "64"))]
type RawSeq = AtomicU64;
/// The publish counter's atomic on a target without 64-bit atomics: a
/// `usize`, so the counter wraps at 2^32 there (`Loan::seq`, "Wrapping").
#[cfg(not(any(loom, target_has_atomic = "64")))]
type RawSeq = AtomicUsize;

/// An atomic publish counter, read and written as a `u64` on every target.
pub(crate) struct AtomicSeq(RawSeq);

impl AtomicSeq {
    pub(crate) fn new(seq: u64) -> Self {
        // Truncates only on a target whose atomic is a `usize`.
        #[allow(clippy::cast_possible_truncation)]
        Self(RawSeq::new(seq as _))
    }

    #[inline]
    pub(crate) fn load(&self, order: Ordering) -> u64 {
        // Widens only on a target whose atomic is a `usize`.
        #[allow(clippy::unnecessary_cast)]
        {
            self.0.load(order) as u64
        }
    }

    pub(crate) fn store(&self, seq: u64, order: Ordering) {
        // Truncates only on a target whose atomic is a `usize`.
        #[allow(clippy::cast_possible_truncation)]
        self.0.store(seq as _, order);
    }
}
