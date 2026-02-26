# heft

Fast disk usage scanner with mtime tracking. Indexes a directory tree into an LMDB database, then queries it instantly.

Similar to [duc](https://duc.zevv.nl/), but faster — heft parallelizes filesystem stat calls across a thread pool and writes results in batched LMDB transactions.

## Install

```bash
cargo install heft
```

## Usage

```bash
# Index a directory
heft index /home/user

# Index without crossing filesystem boundaries
heft index / --one-file-system

# Exclude paths
heft index /home --exclude .cache --exclude target
heft index /home --exclude-from patterns.txt

# Show summary
heft info

# List a directory (sorted by size, largest first)
heft ls
heft ls /home/user/Documents

# Export as JSON
heft json
heft json --compress zstd --output tree.json.zst

# Pack database into a portable .heft file
heft pack
heft pack -o myserver.heft

# Unpack a .heft file into the database
heft unpack myserver.heft

# Inspect a packed file
heft info myserver.heft
```

## Options

| Flag | Description |
|------|-------------|
| `-d, --database <path>` | Database path (default: `~/.heft.db`) |
| `--map-size-gib <n>` | LMDB map size in GiB (default: 4) |
| `--threads <n>` | Worker thread count (default: all cores) |
| `--one-file-system` | Don't cross mount boundaries |
| `--exclude <pattern>` | Exclude paths containing `<pattern>` |
| `--exclude-from <file>` | Read exclude patterns from file |
| `-v, --verbose` | Print each file as it's scanned |
| `-o, --output <file>` | Output file for `pack` and `json` |

## Database

heft stores results in LMDB at `~/.heft.db`. The database is always re-generable via `heft index`, so it uses `WRITE_MAP` + `MAP_ASYNC` flags for speed (no fsync). Override the path with `-d`.

Re-indexing a path only replaces data under that path — if you index `/home` and then `/var`, both coexist in the database. Timestamps are displayed in UTC.

## Notes

- **Network filesystems:** LMDB requires POSIX `fcntl` locking and coherent `mmap`. CIFS, NFS, and other network mounts do not provide these guarantees — store the database on a local filesystem.
- **Database file size:** LMDB pre-allocates a sparse file sized to `--map-size-gib` (default 4 GiB). The file appears large via `ls -l` but actual disk usage is much smaller — use `du` or `heft info` to see the real size.

## AI Disclaimer

This project was built with the assistance of AI tools, specifically **Claude Opus 4.6** (Anthropic) and **Gemini Pro Preview 3.1** (Google). AI-generated code may contain bugs, security issues, or other defects. This software is provided as-is with no warranty. Use at your own risk.

## See also

- [docs/architecture.md](docs/architecture.md) — how heft works internally
- [docs/benchmarks.md](docs/benchmarks.md) — performance notes and profiling
