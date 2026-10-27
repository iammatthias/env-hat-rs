# ADR 0001: time-series storage

Status: accepted 2026-10-25, superseded 2026-10-27 by
[ADR 0003](0003-schema-not-store.md). The `tsdb` crate was removed. This
record stays as history.

## Context

A recorder writes one reading about every 30 s: ~27 numbers, some absent
when a chip fails. The node is a Raspberry Pi Zero 2 W with an SD card
that has already died once from an unclean power loss. Other processes
read at the same time, such as an API that serves a dashboard.

What the store must answer, in order of frequency:
1. The latest value of every field (dashboard tiles).
2. 24 h of one field as a chart (about 200 points).
3. 30 or 90 days of one field, bucketed (min/max/mean/last per bucket).
4. A raw export of a time range.
5. Retention: drop days older than N (default 90).

Constraints:
- Crash safety must be provable by a test that kills the writer mid-write.
- Writes to the SD card should be few, large and sequential.
- The format will gain fields over time, and other node metrics will record
  into it later.
- Dependencies: `libc` only.

## Decision

Pure-Rust append-only store with two file kinds per UTC day:

- `YYYY-MM-DD.log` holds the day's readings. Each frame is self-describing
  and checksummed, with a magic, a version and a CRC-32C. The writer
  appends whole frames and syncs on a cadence. A torn tail is truncated on
  open. A corrupt frame mid-file is skipped by resyncing on the next magic.
- `YYYY-MM-DD.hours` holds per-field hourly summaries (count, sum, min,
  max, last), written once a day is closed. It is derived data: if it is
  missing or stale, it is rebuilt from the log.

Field names are mapped to small ids by define frames inside each log, so
every file stands alone. Retention deletes whole day files. The byte-level
format was in `crates/tsdb/FORMAT.md`, removed with the crate.

## Alternatives considered

Three designs were worked out and compared with this one.

| Alternative | Why it lost |
|---|---|
| SQLite through Debian's libsqlite3 (narrow points table, WAL, hourly rollup table) | Its own estimate was 14–37× write amplification on the SD card (21–56 MB/day for 1.5 MB of data) at full durability. Cross-builds need the Pi's libsqlite3 in a sysroot. Static Alpine builds embed the library, so a CVE means a rebuild. The crash story is excellent but borrowed. |
| Per-day write-ahead log compacted into per-field column files with hourly tables | Sound, and the closest to this decision. Compacting into 27 files a day adds a second codec, a directory-rename commit protocol and 27 unlinks per retained day. That buys per-field reads for data that is under 1 MB a day. The cheap per-field win (hourly summaries) is kept as a derived sidecar. |
| Write-ahead log plus sealed, compressed per-day segments (delta-of-delta timestamps, decimal-scaled values) | Compression shrinks about 1 MB/day to roughly 0.2 MB. The card has 25 GB free. A block codec with an encoder that must verify bit-exact round-trips is the most reader load of any alternative, for a saving that buys nothing here. |

This design takes these ideas from the alternatives:
- CRC-32C with a published check value.
- Telling a torn tail from mid-file corruption by whether a valid frame
  follows.
- An open that is safe to repeat.
- Tests that check the hourly summaries give the same results as the raw
  readings.
- A power-loss test that zero-fills or truncates the file after the last
  synced offset.

## Consequences

- About 0.85 MB/day on disk, about 77 MB for 90 days: the same order as
  the JSONL it replaces. There's no size win, and none is needed. The
  hourly re-defines, which keep a rotted define from costing more than an
  hour of one field, add about 24 KB/day (about 3 %). Backfilled days
  carry them too.
- Clocks: a reading more than `max_future` (default 24 h) ahead of the
  wall clock is refused, so one bad timestamp can't open a far-future day
  and lock out every later reading. Retention counts days from the earlier
  of the newest reading and the wall clock, so it pauses rather than
  deleting history when the clock is behind.
- A 24 h query scans at most two day logs (~1.7 MB). A 90-day query reads
  90 small `.hours` files plus today's log.
- Durability: `append` is atomic. Data is durable at each sync (default
  every 60 s, and on `flush`/drop). A power loss can lose at most the last
  interval. A kill -9 loses nothing the kernel already holds.
- Readers never write, so they need only read access to the store
  directory.
- One writer per directory, enforced with `flock`.
