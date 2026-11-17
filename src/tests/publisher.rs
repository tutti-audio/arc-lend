//! The publishing side's own guarantees: its lock, its buffers, `Debug`.

use super::*;

/// A retired value's destructor runs *after* the control lock is released,
/// so a destructor that panics neither poisons the lock nor leaves the
/// retired list half-updated; the cell keeps working.
///
/// (The other panic site the lock guards against, a failed `reserve`, cannot
/// be provoked here without an allocator that fails on demand; it happens
/// before the swap, which is what makes it harmless.)
#[test]
fn a_panicking_destructor_does_not_poison_the_cell() {
    struct Bomb(bool);
    impl Drop for Bomb {
        fn drop(&mut self) {
            if self.0 {
                panic!("boom");
            }
        }
    }

    let cell = Publisher::new(Bomb(true));
    let mut reader = cell.reader();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        cell.publish(Arc::new(Bomb(false)));
    }));
    assert!(result.is_err(), "the retired value's drop panicked");
    assert!(!cell.cell.control.is_poisoned());
    assert_eq!(cell.retired_len(), 0, "it left the list before it ran");

    cell.publish(Arc::new(Bomb(false)));
    assert!(!reader.lend().0);
    assert_eq!(cell.retired_len(), 0);
}

/// The publisher's buffers are reused: a publish that frees a value leaves
/// the emptied buffer for the next, so a steady stream of publishes stops
/// allocating one per publish.
#[test]
fn a_publish_hands_its_buffer_to_the_next() {
    let cell = Publisher::new(0u32);
    cell.publish(Arc::new(1));
    let first = cell.lock().spare.as_ptr();
    assert!(
        cell.lock().spare.capacity() > 0,
        "the freeing publish kept its buffer"
    );
    cell.publish(Arc::new(2));
    assert_eq!(
        cell.lock().spare.as_ptr(),
        first,
        "the next publish freed into the same buffer and handed it on"
    );
}

/// `Debug` formats the value with the cell's lock released, so a payload
/// whose `fmt` publishes to the same cell (a value that logs a correction
/// of itself) does not deadlock.
#[test]
#[cfg_attr(miri, ignore = "waits on a wall-clock deadline")]
fn debug_formats_outside_the_lock() {
    struct Reentrant {
        cell: Arc<std::sync::Mutex<Option<Publisher<Reentrant>>>>,
    }
    impl std::fmt::Debug for Reentrant {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            if let Some(cell) = self.cell.lock().unwrap().clone() {
                cell.publish(Arc::new(Reentrant {
                    cell: Arc::default(),
                }));
            }
            f.write_str("Reentrant")
        }
    }

    let link = Arc::default();
    let first = Arc::new(Reentrant {
        cell: Arc::clone(&link),
    });
    let cell = Publisher::from_arc(first);
    *link.lock().unwrap() = Some(cell.clone());

    let (done_tx, done_rx) = mpsc::channel();
    let formatter = {
        let cell = cell.clone();
        std::thread::spawn(move || {
            let text = format!("{cell:?}");
            done_tx.send(text).unwrap();
        })
    };
    let text = done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("Debug deadlocked on a payload that publishes");
    formatter.join().unwrap();
    assert_eq!(text, "Publisher(Reentrant)");
    assert_eq!(cell.retired_len(), 0, "the formatted value was freed after");
    // Breaks the cycle the payload made through the publisher.
    link.lock().unwrap().take();
}

/// `current` is the value published last, as the same allocation the
/// publisher handed in, whether it published a `T` or an `Arc<T>`.
#[test]
fn current_is_the_value_published_last() {
    let first = Arc::new(1u32);
    let cell = Publisher::from_arc(Arc::clone(&first));
    assert!(Arc::ptr_eq(&cell.current(), &first));

    let second = Arc::new(2u32);
    cell.publish(Arc::clone(&second));
    assert!(
        Arc::ptr_eq(&cell.current(), &second),
        "an `Arc` it was given"
    );

    cell.publish(3);
    assert_eq!(*cell.current(), 3, "a `T` it wrapped");
}

/// A snapshot from `current` owns its value: a publish retires the cell's
/// own count as usual and pins nothing for it, and the value is freed where
/// the snapshot's last `Arc` drops, here on another thread.
#[test]
fn a_snapshot_owns_its_value_and_frees_where_it_drops() {
    let (sites, make) = sites();
    let cell = Publisher::from_arc(make(0));

    let snapshot = cell.current();
    cell.publish(make(1));
    assert!(freed(&sites).is_empty(), "the snapshot still owns value 0");
    assert_eq!(cell.retired_len(), 0, "and the cell does not pin it");
    assert_eq!(snapshot.id, 0);

    let there = std::thread::spawn(move || {
        drop(snapshot);
        std::thread::current().id()
    })
    .join()
    .unwrap();
    assert_eq!(*sites.lock().unwrap(), vec![(0, there)]);
}

/// A clone of a publisher is another handle on the one cell: what either
/// publishes, both read back, and a reader registered through either lends
/// it. The cell is freed when the last of them goes.
#[test]
fn a_cloned_publisher_is_the_same_cell() {
    let (sites, make) = sites();
    let first = Publisher::from_arc(make(0));
    let second = first.clone();
    let mut reader = second.reader();

    first.publish(make(1));
    assert_eq!(second.current().id, 1);
    second.publish(make(2));
    assert_eq!(first.current().id, 2);
    assert_eq!(reader.lend().id, 2);
    assert_eq!(first.retired_len(), 0, "one cell, one retired list");
    assert_eq!(freed(&sites), vec![0, 1]);

    drop(first);
    assert!(!freed(&sites).contains(&2), "a handle is still alive");
    drop(second);
    assert!(
        !freed(&sites).contains(&2),
        "the reader still holds the cell"
    );
    drop(reader);
    assert_eq!(freed(&sites), vec![0, 1, 2]);
}
