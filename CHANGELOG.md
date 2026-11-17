# Changelog

Every release of arc-lend, newest first. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html): below 1.0, a minor
release may break the API.

## [Unreleased]

## [0.2.0] - Unreleased

The writer's side and the reader's side are now two types. In 0.1 a cell was
shared as `Arc<ArcLend<T>>`, so any holder could publish, reclaim, read the
current value and register readers, and a `Reader` handed the reading thread
that `Arc` back (`Reader::cell`) and registered another reader when cloned.
Nothing in the types kept the thread with the deadline from taking the cell's
lock, allocating or freeing. Now `Publisher<T>` holds every writer-side
operation and a `Reader<T>` holds `lend` alone.

The names follow the roles. `Publisher` is the writer's handle (`publish`,
`reclaim`); `Reader` and `Loan` keep their names. The cell itself is no longer
a public type, so there is nothing to wrap in an `Arc`.

### Added

- `Publisher<T>`: the writer's handle. `Publisher::new(value)` and
  `Publisher::from_arc(arc)` create the cell; `publish`, `reclaim`, `current`,
  `retired_len` and `reader` are its methods, with the behaviour they had on
  `ArcLend`. It is `Send + Sync` exactly when `T: Send + Sync`. It is `Clone`:
  a clone is another handle on the same cell, for a value with several
  control-side owners (they serialize on the cell's lock, as publishers
  already did). It implements `Default` (for `T: Default`) and `Debug`.

### Changed (breaking)

- `ArcLend<T>` is gone, replaced by `Publisher<T>`. The cell is no longer
  shared through `Arc<ArcLend<T>>`; share the `Publisher` by cloning it.
  `Arc::new(ArcLend::new(v))` becomes `Publisher::new(v)`, and
  `Arc::new(ArcLend::from_arc(a))` becomes `Publisher::from_arc(a)`.
- `Publisher::reader(&self)` replaces `ArcLend::reader(self: &Arc<Self>)`.
- A `Reader<T>` can only `lend`. It holds a share of the cell to keep its slot
  alive, and no handle through which to publish, reclaim, read the current
  value or register.
- `Debug` for the cell prints `Publisher(..)` where it printed `ArcLend(..)`.

### Removed

- `Reader::cell`. Keep a `Publisher` (or a clone of it) on the control side
  beside the structure that holds the `Reader`, and call `publish`,
  `current` or `reader` on that. A structure that took
  `Reader::cell()` to hand its cell to a controls handle builds the
  `Publisher` first and registers its reader from it.
- `impl Clone for Reader`. Registering is a writer-side act: it takes the
  lock and may allocate, which a method named `clone` hid on whichever thread
  called it. Call `Publisher::reader` for each additional reader. A type that
  must clone its readers (a node forked on the control thread) keeps a
  `Publisher` clone privately and registers through it in its own `Clone`.

The protocol is unchanged: the slot layout, the fence pair, the freeing rules
and the loom model are the 0.1 ones, and a reader never frees a published value.
The one free that can still land on a reading thread is the cell's own, when
the last handle to it is a `Reader`; keep a `Publisher` for as long as readers
exist.

Handle order, when migrating: keep a `Publisher` alive for as long as any
reader on a deadline thread, or drop those readers off that thread first. A
reader that outlives every `Publisher` frees the cell where it drops.

## [0.1.0] - 2026-11-14

The first release, moved out of the
[tutti](https://github.com/tutti-audio/tutti) repository with its history.

[Unreleased]: https://github.com/tutti-audio/arc-lend/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/tutti-audio/arc-lend/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/tutti-audio/arc-lend/releases/tag/v0.1.0
