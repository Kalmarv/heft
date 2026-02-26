# Benchmarks

## bench.sh

The `bench.sh` script compares heft against [duc](https://duc.zevv.nl/) on the same directory:

```bash
# Usage: sudo ./bench.sh <path> [runs]
sudo ./bench.sh /home/user 3
```

It runs both tools `[runs]` times (default 3), compares file/dir counts to verify correctness, and prints a timing table with averages. `sudo` is needed because duc requires root for some directories.

Example output:

```
  heft vs duc benchmark
  --------------------
  Target:  /home/user
  Runs:    3

  duc        258,423 files      70,713 dirs  82899398656 B
  heft        258,423 files      70,713 dirs  77.2 GiB
  ✓  Stats match

  INDEX SPEED
  -----------------------------------------------
         |     #1     #2     #3  |     avg
  -----------------------------------------------
  duc    |   850ms   820ms   830ms  |   833ms
  heft    |   580ms   560ms   570ms  |   570ms
  -----------------------------------------------
         |                         |   1.46x
```

## Optimization history

### v1: Single-threaded accumulation (baseline)

The main thread iterated jwalk entries one-by-one, accumulated them in an `FxHashMap`, and flushed to LMDB in batches. Flamegraph showed:

- **55%** in jwalk's ordered queue (crossbeam channel contention — one message per file)
- **42%** in main-thread processing (HashMap ops, LMDB read-modify-write merge)
- **<1%** in actual filesystem syscalls

### v2: Worker-thread stat + encoding (current)

Moved all per-entry work into jwalk's `process_read_dir` callback, which runs on rayon worker threads:

- **stat + encode** an entire directory's children inside the callback
- Send **one channel message per directory** instead of per file (~100x fewer messages)
- Write directly to LMDB with `put` (no read-modify-write merge)
- Commit every 1,000 directories

Result: the two userspace hotspots are eliminated. Flamegraph now shows ~91% kernel (filesystem I/O) and ~5% heft userspace — the tool is syscall-bound.

### Key numbers

| Target | Files | Time | Bottleneck |
|--------|-------|------|------------|
| Local SSD (77 GiB, 258k files) | 258,423 | 0.6s | Kernel statx |
| CIFS mount (18 GiB, 88k files) | 88,410 | 5.6s | Network round-trips (91% kernel) |

## Profiling

Generate a flamegraph with [cargo-flamegraph](https://github.com/flamegraph-rs/flamegraph):

```bash
cargo build --release   # debug = true in release profile

# Using perf directly (no sudo needed if perf_event_paranoid <= 1):
perf record -g --call-graph dwarf -F 997 -o /tmp/heft-perf.data \
  -- ./target/release/heft index /home/user

# Convert to flamegraph:
perf script -i /tmp/heft-perf.data \
  | inferno-collapse-perf \
  | inferno-flamegraph > flamegraph.svg

# Quick text summary of hot functions:
perf report -i /tmp/heft-perf.data --stdio --no-children --percent-limit 1
```

The release profile has `debug = true` so perf can resolve function names without affecting optimization.
