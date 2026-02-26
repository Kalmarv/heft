# dux

Fast disk usage scanner with mtime tracking. Indexes a directory tree into an LMDB database, then queries it instantly.

Similar to [duc](https://duc.zevv.nl/), but faster — dux parallelizes filesystem stat calls across a thread pool and writes results in batched LMDB transactions.

## Install

```bash
cargo build --release
# binary at target/release/dux
```

## Usage

```bash
# Index a directory
dux index /home/user

# Index without crossing filesystem boundaries
dux index / --one-file-system

# Exclude paths
dux index /home --exclude .cache --exclude target
dux index /home --exclude-from patterns.txt

# Show summary
dux info

# List a directory (sorted by size, largest first)
dux ls
dux ls /home/user/Documents

# Export as JSON
dux json
dux json --compress zstd --output tree.json.zst
```

## Options

| Flag | Description |
|------|-------------|
| `-d, --database <path>` | Database path (default: `~/.dux.db`) |
| `--threads <n>` | Worker thread count (default: all cores) |
| `--one-file-system` | Don't cross mount boundaries |
| `--exclude <pattern>` | Exclude paths containing `<pattern>` |
| `--exclude-from <file>` | Read exclude patterns from file |
| `-v, --verbose` | Print each file as it's scanned |

## Database

dux stores results in LMDB at `~/.dux.db`. The database is always re-generable via `dux index`, so it uses `WRITE_MAP` + `MAP_ASYNC` flags for speed (no fsync). Override the path with `-d`.

## See also

- [docs/architecture.md](docs/architecture.md) — how dux works internally
- [docs/benchmarks.md](docs/benchmarks.md) — performance notes and profiling
