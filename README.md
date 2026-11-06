# heft

[![CI](https://img.shields.io/github/actions/workflow/status/Kalmarv/heft/ci.yml?branch=main&label=CI)](https://github.com/Kalmarv/heft/actions/workflows/ci.yml) [![release](https://img.shields.io/github/v/release/Kalmarv/heft)](https://github.com/Kalmarv/heft/releases/latest)

Fast disk usage scanner with mtime tracking. Scans a directory tree and produces a portable `.heft` file that can be queried instantly.

Similar to [duc](https://duc.zevv.nl/), but faster — heft parallelizes filesystem stat calls across a thread pool and writes results in batched transactions.

## Install

Download `heft` for Linux or `heft-mft` for Windows (x86_64) from the [latest release](https://github.com/Kalmarv/heft/releases/latest), or build from the repository:

```bash
# Linux / macOS
cargo install --git https://github.com/Kalmarv/heft heft

# Windows (MFT-based scanner)
cargo install --git https://github.com/Kalmarv/heft heft-mft
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

heft produces a single portable `.heft` file per scan. The file contains a zstd-compressed JSON tree with file sizes, mtimes, hardlink metadata, and directory structure. It can be shared, emailed, or inspected without any database.

Internally, heft uses a transient LMDB database during scanning (created in a temporary directory, cleaned up automatically). The database is never exposed to the user.

## Hardlink support

heft stores hardlink metadata (`nlink`, `dev`, `ino`) for every entry. For non-hardlinked files, only `nlink` (1 byte) is stored. For hardlinked files (`nlink > 1`), the device ID and inode number are also recorded so downstream readers can deduplicate.

heft itself is policy-free — it stores raw facts. Readers decide how to handle hardlinks (e.g. dedup with a `HashSet<(dev, ino)>`, annotate, or ignore).

## Notes

- **Default output**: `heft scan /home` → `home.heft`; `heft scan /` → `root.heft`
- **Timestamps** are displayed in UTC.

## AI Disclaimer

This project was built with the assistance of AI tools, specifically **Claude Opus 4.6** (Anthropic) and **Gemini Pro Preview 3.1** (Google). AI-generated code may contain bugs, security issues, or other defects. This software is provided as-is with no warranty. Use at your own risk.

## See also

- [docs/architecture.md](docs/architecture.md) — how heft works internally
- [docs/benchmarks.md](docs/benchmarks.md) — performance notes and profiling
- [CONTRIBUTING.md](CONTRIBUTING.md) — how releases are made
