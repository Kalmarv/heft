# Architecture

## Overview

heft has two phases: **scan** (walk filesystem → produce .heft file) and **query** (read .heft file, display results).

## Scan pipeline

```
Worker threads (rayon)              Main thread
┌──────────────────────┐           ┌───────────────────┐
│ process_read_dir CB  │           │ for batch in rx   │
│  · stat each child   │──channel──│  · db.put(path,   │
│  · encode_many()     │  bounded  │      encoded)     │
│  · send DirBatch     │   (256)   │  · accumulate     │
│                      │           │    counts          │
└──────────────────────┘           └───────────────────┘
        ▲                                  │
   jwalk rayon pool                  commit every 1k dirs
   (readdir + callbacks)
```

1. **jwalk** walks the directory tree using a rayon thread pool. For each directory it reads, it calls our `process_read_dir` callback on a worker thread.
2. The **callback** iterates the directory's children, calls `symlink_metadata()` on each, encodes all entries into a compact binary blob, and sends a `DirBatch` over a bounded crossbeam channel.
3. The **main thread** receives batches and writes them to a transient LMDB database (created in a temporary directory).
4. After scanning completes, `build_tree` reads the LMDB and constructs an in-memory `TreeNode` tree. The tree is then packed into a `.heft` file via `pack_tree`. The temporary LMDB is cleaned up automatically.

## Modules

| Module | Purpose |
|--------|---------|
| `cli.rs` | Clap argument definitions (scan, ls, info) |
| `scanner.rs` | Indexing pipeline (walker + channel + DB writer) |
| `db.rs` | LMDB wrapper (open, read, write, clear) |
| `entry.rs` | Binary encoding/decoding of filesystem entries (LEB128) |
| `tree.rs` | Recursive aggregation for query (build tree, compute sizes) |
| `pack.rs` | Pack/load portable .heft files |
| `error.rs` | Error types |

## Database layout

LMDB is used internally during scanning as a transient store. Keys are absolute directory paths (UTF-8 strings), values are packed binary blobs.

| Key | Value |
|-----|-------|
| `\x00__heft_root__` | Sentinel: which path was scanned |
| `/home/user` | Encoded entries for that directory's children |
| `/home/user/docs` | Encoded entries for docs/ children |

### Entry encoding

Each entry is a variable-length record, concatenated back-to-back with no header:

```
entry_type    (1 byte: 0=File, 1=Dir, 2=Symlink, 3=Other)
mtime         (LEB128 varint)
size_apparent (LEB128 varint)
size_actual   (LEB128 varint)
nlink         (LEB128 varint)
[if nlink > 1]:
  dev         (LEB128 varint)
  ino         (LEB128 varint)
name_len      (LEB128 varint)
name_bytes    (UTF-8, name_len bytes)
```

The `nlink` field stores the hard link count. When `nlink > 1`, the device ID (`dev`) and inode number (`ino`) follow, allowing readers to identify and deduplicate hardlinked files. For non-hardlinked files (`nlink = 1`), `dev` and `ino` are omitted — costing only 1 extra byte per entry.

This format is append-friendly: concatenating two encoded blobs is the same as decoding both, merging, and re-encoding.

## Design decisions

**crossbeam-channel over std::sync::mpsc** — jwalk's `process_read_dir` requires `Fn + Send + Sync`. `std::sync::mpsc::Sender` is `Send` but not `Sync`. crossbeam's `Sender` is both, and jwalk already depends on crossbeam transitively (zero compile cost).

**Channel bound of 256** — prevents unbounded memory growth if the LMDB writer is slower than the walkers. At ~1.5 KB per batch, this caps buffering at ~400 KB.

**WRITE_MAP + MAP_ASYNC** — LMDB writes go directly to an mmap'd region without copying, and pages aren't fsynced on commit. Safe here because the database is transient and always re-generable.

**One stat call per entry** — the callback calls `symlink_metadata()` once per child and extracts file type, size, mtime, device, and block count from the same result.

**Transient LMDB** — the database exists only during `heft scan`, in a temporary directory. This keeps the user experience simple (no `-d` flags, no leftover `.heft.db` directories) while preserving the high-performance batched-write pipeline.

**main.rs imports from lib crate** — `use heft::db` instead of `mod db`. This avoids compiling every module twice (once for lib, once for bin) and ensures the compiler sees cross-crate usage of public methods.

## Packed format (.heft)

A `.heft` file is the only user-facing artifact. It contains a complete directory tree snapshot that can be shared, emailed, or queried with `heft ls` and `heft info`.

### Binary layout

```
Offset  Size      Description
──────  ────      ───────────
0       4 bytes   Magic: "HEFT"
4       1 byte    Format version (0x02)
5       4 bytes   Metadata JSON length (u32 LE)
9       N bytes   Metadata JSON (UTF-8)
9+N     rest      Zstd-compressed tree JSON (compact, not pretty-printed)
```

### Metadata JSON

```json
{
  "root": "/home",
  "timestamp": 1740000000,
  "heft_version": "0.3.1",
  "file_count": 258423,
  "dir_count": 70713,
  "size_actual": 82899398656
}
```

Consumers can read the first 9 + N bytes to extract metadata without decompressing the tree payload.

### Tree JSON

The compressed tree is a recursive `TreeNode` structure. Hardlink fields are conditionally serialized to keep the JSON compact:

- `nlink` — omitted when `<= 1` (most files)
- `dev`, `ino` — omitted when `0`

Old `.heft` files (format v1) lack hardlink data and are rejected with a clear error. Re-scan to produce a v2 file.

### Rationale

- **Magic bytes** — allow `heft info` and other tools to identify the file format by content, not just extension.
- **Separate metadata** — cheap to read without decompressing the tree (useful for file browsers, info commands).
- **Zstd compression** — level 3 gives ~20:1 compression on the JSON tree with fast decompression.
- **Compact JSON** — `to_writer` (not `to_writer_pretty`) saves ~30% on pre-compression size.
- **Conditional hardlink fields** — `dev`/`ino` only appear when `nlink > 1`, keeping the common case (no hardlinks) at zero JSON overhead.
