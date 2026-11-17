//! The shared cell behind a [`Publisher`](crate::Publisher) and its
//! [`Reader`](crate::Reader)s: the current value, and the writer's state.

use std::sync::Arc;

use crate::current::Current;
use crate::slots::Slots;
use crate::sync::{Mutex, MutexGuard, Ordering, fence};

/// State that only the writer's side touches, kept behind the cell's mutex.
/// Only the [`Publisher`](crate::Publisher) reaches it.
pub(crate) struct Control<T> {
    /// Swapped-out values that may still be on loan, oldest first, each with
    /// the sequence number it had while it was current.
    pub(crate) retired: Vec<(Arc<T>, u64)>,
    /// Sequence number of the current value, bumped by every publish.
    pub(crate) seq: u64,
    /// `seq` as of the last scan of the slots.
    pub(crate) scanned_at: u64,
    pub(crate) slots: Slots,
    /// Reused buffer for values about to be freed, so a publish rarely
    /// allocates one. Values are moved in under the lock and dropped after
    /// it.
    pub(crate) spare: Vec<Arc<T>>,
}

impl<T> Control<T> {
    /// Moves into `freeable` every retired value no reader can still be
    /// reading.
    ///
    /// Shared by `publish` and `reclaim`: a fence, one pass over the slots,
    /// and one over the retired list (`docs::design`, §4.5). `freeable` must
    /// have room for all of `retired`. Nothing here allocates or drops, so
    /// nothing can unwind with a value half-moved.
    pub(crate) fn scan(&mut self, freeable: &mut Vec<Arc<T>>) {
        // The publisher half of the fence pair. Orders the swap (or, for a
        // reclaim, the last publish's swap) against every reader's claim; see
        // §5 of `docs::design`.
        fence(Ordering::SeqCst);

        let floor = self.slots.read(self.scanned_at);
        self.scanned_at = self.seq;

        // One order-preserving pass over the retired list, so values are
        // freed oldest first. `extract_if` moves each one at most once;
        // `freeable` has room for the whole list.
        let slots = &self.slots;
        let unpinned = self.retired.extract_if(.., |(value, seq)| {
            *seq < floor && !slots.is_announced(Arc::as_ptr(value).addr())
        });
        freeable.extend(unpinned.map(|(value, _)| value));
    }
}

/// The cell a [`Publisher`](crate::Publisher) and its readers share.
///
/// Crate-private: the publisher is the only handle to the writer's side, and
/// a [`Reader`](crate::Reader) reaches `current` and its own slot alone.
pub(crate) struct Shared<T> {
    pub(crate) current: Current<T>,
    /// Everything a reader never touches: old values, sequence numbers and
    /// the slot list. The mutex also serializes writers. (Declared after
    /// `current`, so the current value is freed before the retired ones.)
    pub(crate) control: Mutex<Control<T>>,
}

impl<T> Shared<T> {
    pub(crate) fn from_arc(value: Arc<T>) -> Self {
        Self {
            current: Current::new(value),
            control: Mutex::new(Control {
                // No capacity reserved: these are only ever touched on the
                // publisher side, where growing them is allowed.
                retired: Vec::new(),
                seq: 0,
                scanned_at: 0,
                slots: Slots::new(),
                spare: Vec::new(),
            }),
        }
    }

    /// Locks the writer's state, ignoring poisoning.
    ///
    /// A poisoned lock still guards consistent state: nothing that runs under
    /// it can panic between changing shared state and making it consistent
    /// again (the only allocations happen before the swap, and destructors
    /// run after the lock is released), so carrying on is correct.
    pub(crate) fn lock(&self) -> MutexGuard<'_, Control<T>> {
        self.control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
