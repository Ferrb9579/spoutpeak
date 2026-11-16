# Patched dependencies

## librqbit 9.1.0

`patches/librqbit` is librqbit 9.1.0 from crates.io, with peer encryption
and a stream scheduler changed for video playback. It replaces the
crates.io version through `[patch.crates-io]` in `Cargo.toml`.

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
- `src/mse.rs`: MSE/PE negotiation and RC4-drop1024 payload transport,
  shared by TCP, uTP and SOCKS connections. `EncryptionMode` offers
  `Disabled` (default), `Prefer` (RC4 first, fresh plaintext retry), and
  `Require` (RC4 only). Incoming MSE resolves the obfuscated torrent hash,
  validates padding and crypto selection, and preserves initial payloads.
- `src/stream_connect.rs`, `src/peer_connection.rs`, `src/session.rs`:
  apply encryption to ordinary peers and magnet metadata discovery.
  `SessionOptions::encryption` sets the initial policy;
  `Session::set_encryption` disconnects existing peers when it changes.
  Prefer also accepts MSE plaintext from peers that offer no RC4; Require
  rejects plaintext connections and plaintext MSE payloads.

MSE/PE obfuscates peer traffic. It does not hide IP addresses, encrypt
tracker/DHT traffic, or encrypt the cache on disk.

Tests for both are at the end of `src/piece_tracker.rs`. Run them with
`cargo test --lib --target-dir ../../target/librqbit-test` from
`patches/librqbit`.

Encryption tests (from the repository root):

```powershell
cargo test --manifest-path patches/librqbit/Cargo.toml --no-default-features --features rust-tls --lib mse::tests --target-dir target/mse-tests
cargo test --manifest-path patches/librqbit/Cargo.toml --no-default-features --features rust-tls --lib encryption --target-dir target/mse-tests
```

The optional `libtorrent_interoperability` test requires Python with the
`libtorrent` module. Set `SPOUTPEAK_LIBTORRENT_PYTHON` to its executable
(default `python`) and, if needed, `SPOUTPEAK_LIBTORRENT_PYTHONPATH` to the
module directory. Run the same command with the filter
`libtorrent_interoperability` and append `-- --ignored --nocapture`.
It verifies RC4 flags and complete payloads in both directions over TCP
and uTP, including encrypted magnet metadata discovery.

When upgrading librqbit, copy the new release over this folder and re-apply
the changes above (or drop the patch if upstream has stream priorities).
