//! [`Publisher`], and everything that runs on the writer's side: publishing,
//! reclaiming, registering readers.

use std::sync::Arc;

use crate::reader::Reader;
use crate::shared::Shared;

/// The writer's handle to a shared value that readers borrow and only the
/// writer frees.
///
/// One thread replaces the value with [`publish`](Self::publish). Each thread
/// that reads it gets a [`Reader`] from [`reader`](Self::reader), once, and
/// borrows the value through it with [`Reader::lend`]. A read is wait-free,
/// allocation-free and free-free: it returns a [`Loan`](crate::Loan), a
/// borrow whose drop cannot run a destructor. Old values are freed by
/// `publish` or [`reclaim`](Self::reclaim), on the writer's thread.
///
/// Use it for state too large for an atomic: a routing table, a set of filter
/// coefficients, a level's collision data.
///
/// # Two sides, two types
///
/// Everything that locks, allocates or frees is a method of the `Publisher`:
/// [`publish`](Self::publish), [`reclaim`](Self::reclaim),
/// [`current`](Self::current), [`seq`](Self::seq),
/// [`retired_len`](Self::retired_len) and [`reader`](Self::reader). A
/// [`Reader`] has [`lend`](Reader::lend) and nothing else, and holds no
/// `Publisher`, so the thread that reads cannot reach a writer-side call.
///
/// # Cloning
///
/// A clone is another handle on the same cell, for a value with several
/// control-side owners. Publishers serialize on the cell's lock: the value
/// is the latest publish of any of them.
///
/// # Dropping the cell
///
/// Each [`Reader`] holds a share of the cell, so whichever handle drops last
/// frees the cell, with its current value and any old ones still waiting. If
/// that is a reader, the free happens there. Keep a `Publisher` for as long
/// as readers exist.
///
/// What each operation costs is in [`docs::design`](crate::docs::design),
/// §7.
///
/// # Examples
///
/// ```
/// use arc_lend::Publisher;
///
/// struct Settings { gains: Vec<f32> }
///
/// // 1. The cell, and its publisher.
/// let publisher = Publisher::new(Settings { gains: vec![1.0; 4] });
///
/// // 2. Register the reader on the writer's side, then move it to its thread.
/// let mut reader = publisher.reader();
/// let reader_thread = std::thread::spawn(move || {
///     for _iteration in 0..1000 {
///         // 3. One lend per iteration: wait-free, allocation-free, free-free.
///         let settings = reader.lend();
///         let total: f32 = settings.gains.iter().sum();
///     }
/// });
///
/// // 4. The writer publishes new versions, and frees old ones here.
/// for n in 1..100 {
///     publisher.publish(Settings { gains: vec![0.5; 4 + n % 3] });
/// }
/// reader_thread.join().unwrap();
/// ```
pub struct Publisher<T> {
    pub(crate) cell: Arc<Shared<T>>,
}

impl<T> Publisher<T> {
    /// Creates a cell holding `value`, and returns its publisher.
    ///
    /// Allocates the `Arc` that carries the value.
    pub fn new(value: T) -> Self {
        Self::from_arc(Arc::new(value))
    }

    /// Creates a cell holding an existing `Arc`, and returns its publisher.
    ///
    /// The cell takes over that strong count. Other clones the caller keeps
    /// are unaffected, and the value is freed wherever its last `Arc` drops:
    /// never on a reader, since readers hold none.
    pub fn from_arc(value: Arc<T>) -> Self {
        Self {
            cell: Arc::new(Shared::from_arc(value)),
        }
    }

    /// Registers a new [`Reader`] of this cell.
    ///
    /// Call it on the writer's side, ahead of time: it takes the cell's lock,
    /// and may allocate. Then move the reader to the thread that reads. The
    /// reader holds a share of the cell, so it can live in a long-lived
    /// structure. To read on two threads, or at two levels of nesting,
    /// register one reader for each.
    ///
    /// Each reader keeps a slot in the cell until it drops, and the next
    /// registration reuses it. The cell never gives slots back (see
    /// [`docs::guide`](crate::docs::guide)).
    ///
    /// A reader registered and dropped at once is a mistake, and does not
    /// compile under `deny(unused_must_use)`:
    ///
    /// ```compile_fail
    /// #![deny(unused_must_use)]
    /// let publisher = arc_lend::Publisher::new(1u32);
    /// publisher.reader(); // registered, then dropped: nothing will lend
    /// ```
    #[must_use = "a reader dropped at once frees the slot it registered; keep it to lend"]
    pub fn reader(&self) -> Reader<T> {
        let slot = self.cell.lock().slots.register();
        Reader::new(Arc::clone(&self.cell), slot)
    }

    /// Replaces the value, and frees old values no reader is still using.
    ///
    /// Takes a `T`, or an `Arc<T>` the caller already holds (the cell takes
    /// over that strong count, as in [`from_arc`](Self::from_arc)). Call it
    /// on the writer's thread only: it locks, may allocate, and runs
    /// destructors.
    ///
    /// It never waits on a reader. An old value a reader still holds, or may
    /// be about to read, is kept and checked again by the next `publish` or
    /// [`reclaim`](Self::reclaim). The frees run after the lock is released.
    ///
    /// # Panics
    ///
    /// If an old value's destructor panics, the panic reaches the caller once
    /// the lock is released; the cell stays usable, holding the new value.
    pub fn publish(&self, value: impl Into<Arc<T>>) {
        // Wrapping a `T` allocates, here, before the lock: if it panics,
        // nothing has changed.
        let value = value.into();
        let freeable = {
            let mut control = self.cell.lock();
            let control = &mut *control;

            // The only allocations, before anything changes. If one panics,
            // nothing has; `value` is dropped on this thread by the unwind,
            // and no reader has seen it.
            control.retired.reserve(1);
            let mut freeable = std::mem::take(&mut control.spare);
            freeable.reserve(control.retired.len() + 1);

            // Swap, under the lock (see `Current::swap`), and retire the
            // outgoing value. The push cannot allocate (reserved above).
            let old_seq = control.seq;
            control.seq += 1;
            let old = self.cell.current.swap(value, control.seq);
            control.retired.push((old, old_seq));

            control.scan(&mut freeable);
            freeable
        };
        self.free(freeable);
    }

    /// Returns the current value as an owning `Arc`.
    ///
    /// Useful for deriving the next value from the last, or for starting a
    /// copy of a structure that reads the cell, without keeping a copy of
    /// the value beside it.
    ///
    /// It takes the cell's lock, and the `Arc` frees the value wherever it is
    /// dropped last. A reader [`lend`](Reader::lend)s instead. Holding the
    /// `Arc` does not hold up the cell: publishing carries on as usual.
    #[must_use]
    pub fn current(&self) -> Arc<T> {
        self.cell.current.to_arc(&self.cell.lock())
    }

    /// Returns the publish number of the current value.
    ///
    /// The value given to [`new`](Self::new) or [`from_arc`](Self::from_arc)
    /// is number 0, and each [`publish`](Self::publish) makes the next
    /// number. It takes the cell's lock, so it is exact: a [`Loan`](crate::Loan)
    /// taken afterwards reports this number or a later one
    /// ([`Loan::seq`](crate::Loan::seq)). On a target without 64-bit atomics
    /// a loan's number is truncated to `usize::BITS` bits and this one is not,
    /// so they agree only until the count passes 2^`usize::BITS`.
    ///
    /// ```
    /// use arc_lend::Publisher;
    ///
    /// let publisher = Publisher::new("a");
    /// assert_eq!(publisher.seq(), 0);
    /// publisher.publish("b");
    /// assert_eq!(publisher.seq(), 1);
    /// let mut reader = publisher.reader();
    /// assert_eq!(reader.lend().seq(), 1);
    /// ```
    #[must_use]
    pub fn seq(&self) -> u64 {
        self.cell.lock().seq
    }

    /// Frees old values no reader is still using, without publishing.
    ///
    /// Returns how many old values are still kept afterwards, because a
    /// reader holds one or might be about to read it. Call it from a periodic
    /// task on the writer's side, to get memory back when there is nothing
    /// new to publish. Like [`publish`](Self::publish), it never waits on a
    /// reader.
    ///
    /// When nothing is waiting it is only a lock and a length check, so it
    /// can be called every tick, for every cell.
    ///
    /// # Panics
    ///
    /// If an old value's destructor panics, as for [`publish`](Self::publish).
    pub fn reclaim(&self) -> usize {
        let (freeable, pinned) = {
            let mut control = self.cell.lock();
            let control = &mut *control;
            // Nothing to free, and skipping the scan keeps exactly what
            // scanning would (`docs::design`, §4.4).
            if control.retired.is_empty() {
                return 0;
            }
            let mut freeable = std::mem::take(&mut control.spare);
            freeable.reserve(control.retired.len());
            control.scan(&mut freeable);
            (freeable, control.retired.len())
        };
        self.free(freeable);
        pinned
    }

    /// Drops `freeable` outside the lock, then keeps its buffer for the next
    /// publish.
    ///
    /// `T::drop` is arbitrary code: one that publishes to this cell, or
    /// panics, must not run under our mutex.
    fn free(&self, mut freeable: Vec<Arc<T>>) {
        freeable.clear();
        // Never waits: if another publisher holds the lock, this buffer is
        // dropped instead and the next publish allocates one.
        if let Ok(mut control) = self.cell.control.try_lock()
            && control.spare.capacity() < freeable.capacity()
        {
            control.spare = freeable;
        }
    }

    /// Returns how many old values are waiting to be freed.
    ///
    /// It takes the cell's lock. Loans keep at most one value per [`Reader`]
    /// waiting. A higher count means a reader was paused mid-read while
    /// values were published, and the next `publish` or
    /// [`reclaim`](Self::reclaim) after it resumes frees them (see
    /// [`docs::guide`](crate::docs::guide)). What to do about a count that
    /// stays high is the caller's policy: the cell itself neither asserts nor
    /// logs.
    #[must_use]
    pub fn retired_len(&self) -> usize {
        self.cell.lock().retired.len()
    }
}

impl<T> Clone for Publisher<T> {
    /// Another handle on the same cell: no value is copied, and nothing
    /// locks or allocates beyond the reference count.
    fn clone(&self) -> Self {
        Self {
            cell: Arc::clone(&self.cell),
        }
    }
}

impl<T: Default> Default for Publisher<T> {
    fn default() -> Self {
        Self::new(T::default())
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for Publisher<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Take a strong count under the lock, then format with the lock
        // released: a `T::fmt` that publishes to this cell would otherwise
        // deadlock on it.
        let value = self.current();
        f.debug_tuple("Publisher").field(&*value).finish()
    }
}
