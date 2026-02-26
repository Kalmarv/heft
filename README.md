# heft

Fast disk usage scanner with mtime tracking. Scans a directory tree and produces a portable `.heft` file that can be queried instantly.

Similar to [duc](https://duc.zevv.nl/), but faster — heft parallelizes filesystem stat calls across a thread pool and writes results in batched transactions.

## Install

```bash
cargo install heft
```

## Usage

```bash
# Scan a directory → produce a .heft file
heft scan /home/user
heft scan /home/user -o myserver.heft

# Scan without crossing filesystem boundaries
heft scan / --one-file-system

# Exclude paths
heft scan /home --exclude .cache --exclude target
heft scan /home --exclude-from patterns.txt

# List contents (sorted by size, largest first)
heft ls home.heft
heft ls home.heft /home/user/Documents

# Show metadata
heft info home.heft
```

## Options

| Flag | Description |
|------|-------------|
| `--threads <n>` | Worker thread count (default: all cores) |
| `--one-file-system` | Don't cross mount boundaries |
| `--exclude <pattern>` | Exclude paths containing `<pattern>` |
| `--exclude-from <file>` | Read exclude patterns from file |
| `-v, --verbose` | Print each file as it's scanned |
| `-o, --output <file>` | Output file for `scan` (default: `<basename>.heft`) |

## .heft files

heft produces a single portable `.heft` file per scan. The file contains a zstd-compressed JSON tree with file sizes, mtimes, and directory structure. It can be shared, emailed, or inspected without any database.

Internally, heft uses a transient LMDB database during scanning (created in a temporary directory, cleaned up automatically). The database is never exposed to the user.

## Notes

- **Default output**: `heft scan /home` → `home.heft`; `heft scan /` → `root.heft`
- **Timestamps** are displayed in UTC.

## AI Disclaimer

This project was built with the assistance of AI tools, specifically **Claude Opus 4.6** (Anthropic) and **Gemini Pro Preview 3.1** (Google). AI-generated code may contain bugs, security issues, or other defects. This software is provided as-is with no warranty. Use at your own risk.

## See also

- [docs/architecture.md](docs/architecture.md) — how heft works internally
- [docs/benchmarks.md](docs/benchmarks.md) — performance notes and profiling
