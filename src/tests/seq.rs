//! The publish number: `Publisher::seq` and `Loan::seq`.

use super::*;

/// The first value is number 0 and each publish makes the next, so the
/// publisher's number and a fresh loan's agree after every step.
#[test]
fn the_number_starts_at_zero_and_steps_by_one_per_publish() {
    let cell = Publisher::new(0u64);
    let mut reader = cell.reader();
    assert_eq!(cell.seq(), 0);
    assert_eq!(reader.lend().seq(), 0);
    for n in 1..=20u64 {
        cell.publish(n);
        assert_eq!(cell.seq(), n);
        let loan = reader.lend();
        assert_eq!((loan.seq(), *loan), (n, n));
    }
}

/// `from_arc` takes an existing `Arc` as number 0, and a publish of an `Arc`
/// counts like a publish of a `T`.
#[test]
fn from_arc_starts_at_zero_and_a_published_arc_counts() {
    let first = Arc::new(7u32);
    let cell = Publisher::from_arc(first.clone());
    let mut reader = cell.reader();
    assert_eq!(cell.seq(), 0);
    assert_eq!(reader.lend().seq(), 0);
    cell.publish(Arc::new(8));
    cell.publish(9);
    let loan = reader.lend();
    assert_eq!((loan.seq(), *loan), (2, 9));
}

/// A reclaim publishes nothing, so it does not move the number, whether or
/// not it frees anything.
#[test]
fn a_reclaim_does_not_move_the_number() {
    let cell = Publisher::new(0u8);
    let mut reader = cell.reader();
    cell.reclaim();
    cell.publish(1);
    let held = reader.lend();
    cell.publish(2);
    assert_eq!(cell.reclaim(), 1, "the held value is kept");
    assert_eq!(cell.seq(), 2);
    drop(held);
    assert_eq!(cell.reclaim(), 0);
    assert_eq!(cell.seq(), 2);
    assert_eq!(reader.lend().seq(), 2);
}

/// Clones of a publisher are one cell: they count together.
#[test]
fn clones_of_a_publisher_share_one_count() {
    let a = Publisher::new(0u8);
    let b = a.clone();
    a.publish(1);
    b.publish(2);
    a.publish(3);
    assert_eq!((a.seq(), b.seq()), (3, 3));
    assert_eq!(a.reader().lend().seq(), 3);
}

/// A loan reports the number its lend read for as long as it lives, whatever
/// is published after it, and the next lend reports the newer one.
#[test]
fn a_held_loan_keeps_its_number() {
    let cell = Publisher::new(10u32);
    let mut reader = cell.reader();
    let held = reader.lend();
    cell.publish(11);
    cell.publish(12);
    assert_eq!((held.seq(), *held), (0, 10));
    drop(held);
    let next = reader.lend();
    assert_eq!((next.seq(), *next), (2, 12));
}

/// Every reader of a cell reports the same number for the same publish, and
/// a reader that does not lend between publishes sees the whole step.
#[test]
fn readers_agree_on_the_number() {
    let cell = Publisher::new(0u32);
    let mut readers: Vec<_> = (0..5).map(|_| cell.reader()).collect();
    for n in 1..=3 {
        cell.publish(n);
    }
    for reader in &mut readers {
        let loan = reader.lend();
        assert_eq!((loan.seq(), *loan), (3, 3));
    }
}

/// The idiom the docs show: comparing each lend's number with the last one
/// seen fires once per change at least, and for a publish that lands before
/// the lend it fires on that lend, so a quiescent reader never misses one.
#[test]
fn comparing_numbers_detects_every_publish_between_lends() {
    let cell = Publisher::new(0u32);
    let mut reader = cell.reader();
    let mut last = reader.lend().seq();
    let mut changes = 0;
    for round in 0..50u32 {
        // Zero, one or two publishes between lends.
        for k in 0..round % 3 {
            cell.publish(round * 10 + k);
        }
        let loan = reader.lend();
        let changed = loan.seq() != last;
        assert_eq!(changed, round % 3 != 0, "round {round}");
        changes += u32::from(changed);
        last = loan.seq();
        assert_eq!(last, cell.seq());
    }
    assert_eq!(changes, 33);
}

/// The atomic a lend reads is the counter under the lock: the publisher
/// writes it with every publish, and a reclaim leaves both alone.
#[test]
fn the_atomic_mirrors_the_counter_under_the_lock() {
    let cell = Publisher::new(0u8);
    for n in 1..=5 {
        cell.publish(n);
        assert_eq!(cell.cell.current.load_seq(), cell.lock().seq);
        cell.reclaim();
        assert_eq!(cell.cell.current.load_seq(), u64::from(n));
    }
}

/// Past 32 bits the number is exact on a target with 64-bit atomics. (Where
/// the atomic is a `usize` the number wraps, and this test is compiled out.)
#[cfg(target_has_atomic = "64")]
#[test]
fn the_number_is_a_full_u64() {
    let cell = Publisher::new(0u8);
    let big = u64::from(u32::MAX) + 5;
    cell.lock().seq = big - 1;
    cell.publish(1);
    assert_eq!(cell.seq(), big);
    assert_eq!(cell.reader().lend().seq(), big);
}

/// Where the atomic is a `usize` a loan's number is the count modulo the
/// width of `usize` (2^32 on a 32-bit target), and `Publisher::seq` stays
/// the full count.
#[cfg(not(target_has_atomic = "64"))]
#[test]
fn a_loan_number_wraps_at_usize_without_64_bit_atomics() {
    let cell = Publisher::new(0u8);
    let big = u64::from(u32::MAX) + 5;
    cell.lock().seq = big - 1;
    cell.publish(1);
    assert_eq!(cell.seq(), big);
    let mask = u64::try_from(usize::MAX).unwrap();
    assert_eq!(cell.reader().lend().seq(), big & mask);
}

/// Readers on threads against a publisher that publishes the numbers `1..=publishes`
/// as its values: every loan's number is at most its value (the number is
/// never newer than the value), neither goes down from one lend of a reader to
/// the next, and once the publisher has finished, a reader's next lend is the
/// final pair, so a reader that compares numbers sees the last change.
fn numbers_against_a_publisher(readers: usize, publishes: u64) {
    let cell = Publisher::new(0u64);
    let done = Arc::new(StdAtomicBool::new(false));

    let handles: Vec<_> = (0..readers)
        .map(|_| {
            let done = done.clone();
            let mut reader = cell.reader();
            std::thread::spawn(move || {
                let (mut last_seq, mut last_value) = (0, 0);
                let mut changes = 0u64;
                let mut check = |reader: &mut Reader<u64>| {
                    let loan = reader.lend();
                    let (seq, value) = (loan.seq(), *loan);
                    assert!(value >= seq, "seq {seq} is newer than its value {value}");
                    assert!(seq >= last_seq, "seq went back: {seq} after {last_seq}");
                    assert!(value >= last_value, "value went back: {value}");
                    changes += u64::from(seq != last_seq);
                    (last_seq, last_value) = (seq, value);
                };
                // `Acquire` pairs with the publisher's `Release` store of
                // `done`, which follows its last publish.
                while !done.load(StdOrdering::Acquire) {
                    check(&mut reader);
                }
                // The publisher has finished: this lend is the final pair.
                check(&mut reader);
                assert_eq!((last_seq, last_value), (publishes, publishes));
                assert!(changes <= publishes, "more changes than publishes");
                assert!(changes >= 1 || publishes == 0, "missed every change");
            })
        })
        .collect();

    for n in 1..=publishes {
        cell.publish(n);
    }
    assert_eq!(cell.seq(), publishes);
    done.store(true, StdOrdering::Release);
    for handle in handles {
        handle.join().unwrap();
    }
}

/// Four readers against a publisher publishing three million times: about a second
/// in release.
#[test]
#[cfg_attr(miri, ignore = "threaded stress test; miri runs the small one below")]
fn numbers_hold_under_a_hammering_publisher() {
    let publishes = if cfg!(debug_assertions) {
        100_000
    } else {
        3_000_000
    };
    numbers_against_a_publisher(4, publishes);
}

/// The same, small enough for miri's weak-memory emulation.
#[test]
fn numbers_hold_under_a_small_publisher() {
    numbers_against_a_publisher(2, 40);
}
