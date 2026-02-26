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
```

## Options

| Flag | Description |
|------|-------------|
| `-d, --database <path>` | Database path (default: `~/.heft.db`) |
| `--threads <n>` | Worker thread count (default: all cores) |
| `--one-file-system` | Don't cross mount boundaries |
| `--exclude <pattern>` | Exclude paths containing `<pattern>` |
| `--exclude-from <file>` | Read exclude patterns from file |
| `-v, --verbose` | Print each file as it's scanned |

## Database

heft stores results in LMDB at `~/.heft.db`. The database is always re-generable via `heft index`, so it uses `WRITE_MAP` + `MAP_ASYNC` flags for speed (no fsync). Override the path with `-d`.

## AI Disclaimer

This project was built with the assistance of AI tools, specifically **Claude Opus 4.6** (Anthropic) and **Gemini Pro Preview 3.1** (Google). AI-generated code may contain bugs, security issues, or other defects. This software is provided as-is with no warranty. Use at your own risk.

## See also

- [docs/architecture.md](docs/architecture.md) — how heft works internally
- [docs/benchmarks.md](docs/benchmarks.md) — performance notes and profiling
