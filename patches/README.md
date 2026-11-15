# Patched dependencies

## librqbit 9.1.0

`patches/librqbit` is librqbit 9.1.0 from crates.io, with the stream
scheduler changed for video playback. It replaces the crates.io version
through `[patch.crates-io]` in `Cargo.toml`.

Upstream treats every open stream the same: each one asks for the 32 MiB
after its position, and all those windows are shuffled together. A player,
our read-ahead, a probe and an end-of-file reader therefore compete equally,
and pieces arrive scattered instead of from the playback position on.
Fast peers also steal the slowest piece in flight, whatever it is, before
the stream pieces are even looked at.

Changes:
- `src/torrent_state/streaming.rs`: `StreamPriority` (`Background`,
  `Normal`, `Playback`) and `FileStream::set_priority`, `set_window` (bytes
  past the position to fetch) and `set_position_now` (move the window
  without reading). `iter_next_pieces` orders streams by priority, then
  newest first, and interleaves pieces only within a priority; no shuffle.
- `src/piece_tracker.rs`: stream pieces are collected once per
  acquisition. The first four missing ones are stolen from a peer that has
  held one for twice this peer's average piece time (at least 1.5 s); other
  steals pick the piece a stream needs soonest before the oldest one.
- Exports `FileStream` and `StreamPriority` from the crate root.

Tests for both are at the end of `src/piece_tracker.rs`. Run them with
`cargo test --lib --target-dir ../../target/librqbit-test` from
`patches/librqbit`.

When upgrading librqbit, copy the new release over this folder and re-apply
the changes above (or drop the patch if upstream has stream priorities).
