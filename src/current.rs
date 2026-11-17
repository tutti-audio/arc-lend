//! Pointer to the cell's current value, and every `unsafe` that turns it
//! back into an `Arc`.

use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::shared::Control;
use crate::slots::Padded;
use crate::sync::{AtomicPtr, AtomicSeq, Ordering};

/// What a lend reads and a publish writes: the pointer and its mirror of the
/// publish counter.
struct Published<T> {
    ptr: AtomicPtr<T>,
    /// The publish number of the value `ptr` holds, or of an earlier one
    /// while a publish is between its swap and this store. Written only
    /// after the swap, under the control lock.
    seq: AtomicSeq,
}

/// Atomic pointer to the current value, made by [`Arc::into_raw`], and the
/// publish number beside it.
///
/// The cell owns the value's one strong count through it. Every `unsafe`
/// that turns the pointer back into an `Arc` is here;
/// [`Loan`](crate::Loan)'s dereference is the one outside.
pub(crate) struct Current<T> {
    /// On a line of its own, the pointer and the counter together, so a lend
    /// reads one line: every lend reads them, and only a publish writes them.
    ptr: Padded<Published<T>>,
    /// Owns an `Arc<T>` through a raw pointer; this gives it (and so the
    /// cell) `Arc<T>`'s auto traits (`Send + Sync` exactly when
    /// `T: Send + Sync`) and drop-check behaviour, so no `unsafe impl` is
    /// needed.
    _owns: PhantomData<Arc<T>>,
}

/// Gives up `value`'s strong count as the raw pointer the cell stores.
///
/// A slot tells an announcement from a claim by the low bit, so every stored
/// pointer must be even. An `Arc`'s data sits after its two counts, so its
/// address is aligned to at least a `usize`; this checks it once, on the
/// control side, before any reader can load the pointer.
///
/// # Panics
///
/// If the pointer is odd, which no `Arc` layout std has used produces.
fn into_stored<T>(value: Arc<T>) -> *mut T {
    assert!(
        Arc::as_ptr(&value).addr() & 1 == 0,
        "an `Arc`'s data pointer is even"
    );
    Arc::into_raw(value).cast_mut()
}

impl<T> Current<T> {
    pub(crate) fn new(value: Arc<T>) -> Self {
        Self {
            ptr: Padded(Published {
                ptr: AtomicPtr::new(into_stored(value)),
                seq: AtomicSeq::new(0),
            }),
            _owns: PhantomData,
        }
    }

    /// Loads the current pointer, for a lend.
    ///
    /// `Acquire` pairs with [`swap`](Self::swap)'s `Release`, so the pointee
    /// is fully constructed from the loading thread's point of view.
    #[inline]
    pub(crate) fn load(&self) -> NonNull<T> {
        let ptr = self.ptr.0.ptr.load(Ordering::Acquire);
        // Only ever set from `Arc::into_raw`, never null.
        NonNull::new(ptr).expect("Publisher::current is never null")
    }

    /// Loads the publish number, for a lend, before it loads the pointer.
    ///
    /// `Acquire` pairs with [`swap`](Self::swap)'s `Release` store of the
    /// number, which follows its swap of the pointer: a load of the pointer
    /// sequenced after this one reads the value of that number or a later
    /// one (`docs::design`, §5.2).
    #[inline]
    pub(crate) fn load_seq(&self) -> u64 {
        self.ptr.0.seq.load(Ordering::Acquire)
    }

    /// Swaps `value` in as publish number `seq`, and returns the outgoing
    /// value for the caller to retire.
    ///
    /// Readers may still hold the outgoing value: dropping what this returns
    /// is the caller's decision, made by the scan.
    ///
    /// `Release` publishes the new value's contents to readers' `Acquire`
    /// loads. No `Acquire` half: the outgoing value was stored by a publisher
    /// holding the control lock, which the caller holds too, so the mutex
    /// already orders its contents before us, and what readers did with it
    /// reaches the free through the scan's `Acquire` slot reads. (`AcqRel`
    /// here was redundant with the lock.)
    ///
    /// The number is stored after the swap, `Release`, so a reader that loads
    /// it loads the pointer of that value or a later one.
    pub(crate) fn swap(&self, value: Arc<T>, seq: u64) -> Arc<T> {
        let old = self.ptr.0.ptr.swap(into_stored(value), Ordering::Release);
        self.ptr.0.seq.store(seq, Ordering::Release);
        // SAFETY: `old` came out of `ptr`, which only ever holds a pointer
        // from `Arc::into_raw` whose strong count the cell owns. The swap
        // removed it, so this is the one place that count is reclaimed.
        unsafe { Arc::from_raw(old) }
    }

    /// Returns a new strong count on the current value.
    ///
    /// The caller holds the cell's lock (`_locked` is the proof), so no
    /// publish can retire and free the value between the load and the
    /// increment.
    pub(crate) fn to_arc(&self, _locked: &Control<T>) -> Arc<T> {
        let ptr = self.load().as_ptr();
        // SAFETY: `ptr` is from `Arc::into_raw` with a count the cell owns,
        // and only a publish, which needs the lock the caller holds, can
        // replace (and so retire) it. The increment gives `from_raw` a count
        // of its own to take.
        unsafe {
            Arc::increment_strong_count(ptr);
            Arc::from_raw(ptr)
        }
    }
}

impl<T> Drop for Current<T> {
    fn drop(&mut self) {
        // Dropped only with the cell. `&mut` means no `Reader` is alive (each
        // holds an `Arc` of the cell), so no loan is, and no atomic access is
        // needed.
        #[cfg(not(loom))]
        let ptr = *self.ptr.0.ptr.get_mut();
        #[cfg(loom)]
        let ptr = self.ptr.0.ptr.with_mut(|p| *p);
        // SAFETY: as in `swap`: `ptr` holds a pointer from `Arc::into_raw`
        // whose count the cell owns, and nothing will read it again.
        drop(unsafe { Arc::from_raw(ptr) });
    }
}
