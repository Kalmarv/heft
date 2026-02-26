# Architecture

## Overview

heft has two phases: **index** (walk filesystem, write to DB) and **query** (read DB, display results).

## Indexing pipeline

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
3. The **main thread** receives batches and writes them to LMDB. It commits the write transaction every 1,000 directories.
4. A separate **driver thread** iterates the walker to completion (its return values are discarded — the callback does the real work).

## Modules

| Module | Purpose |
|--------|---------|
| `cli.rs` | Clap argument definitions |
| `scanner.rs` | Indexing pipeline (walker + channel + DB writer) |
| `db.rs` | LMDB wrapper (open, read, write, clear) |
| `entry.rs` | Binary encoding/decoding of filesystem entries (LEB128) |
| `tree.rs` | Recursive aggregation for query (build tree, compute sizes) |
| `export.rs` | JSON export with optional compression |
| `pack.rs` | Pack/unpack portable .heft files |
| `error.rs` | Error types |

## Database layout

LMDB with a single `dirs` database. Keys are absolute directory paths (UTF-8 strings), values are packed binary blobs.

| Key | Value |
|-----|-------|
| `\x00__heft_root__` | Sentinel: which path was indexed |
| `/home/user` | Encoded entries for that directory's children |
| `/home/user/docs` | Encoded entries for docs/ children |

### Entry encoding

Each entry is a variable-length record, concatenated back-to-back with no header:

```
entry_type    (1 byte: 0=File, 1=Dir, 2=Symlink, 3=Other)
mtime         (LEB128 varint)
size_apparent (LEB128 varint)
size_actual   (LEB128 varint)
name_len      (LEB128 varint)
name_bytes    (UTF-8, name_len bytes)
```

This format is append-friendly: concatenating two encoded blobs is the same as decoding both, merging, and re-encoding.

## Design decisions

**crossbeam-channel over std::sync::mpsc** — jwalk's `process_read_dir` requires `Fn + Send + Sync`. `std::sync::mpsc::Sender` is `Send` but not `Sync`. crossbeam's `Sender` is both, and jwalk already depends on crossbeam transitively (zero compile cost).

**Channel bound of 256** — prevents unbounded memory growth if the LMDB writer is slower than the walkers. At ~1.5 KB per batch, this caps buffering at ~400 KB.

**WRITE_MAP + MAP_ASYNC** — LMDB writes go directly to an mmap'd region without copying, and pages aren't fsynced on commit. Safe here because the database is always re-generable.

**One stat call per entry** — the callback calls `symlink_metadata()` once per child and extracts file type, size, mtime, device, and block count from the same result.

**main.rs imports from lib crate** — `use heft::db` instead of `mod db`. This avoids compiling every module twice (once for lib, once for bin) and ensures the compiler sees cross-crate usage of public methods.

## Packed format (.heft)

A `.heft` file is a single portable file containing a complete directory tree snapshot. It can be shared, emailed, or dragged into a treemap visualizer without requiring LMDB.

### Binary layout

```
Offset  Size      Description
──────  ────      ───────────
0       4 bytes   Magic: "HEFT"
4       1 byte    Format version (0x01)
5       4 bytes   Metadata JSON length (u32 LE)
9       N bytes   Metadata JSON (UTF-8)
9+N     rest      Zstd-compressed tree JSON (compact, not pretty-printed)
```

### Metadata JSON

```json
{
  "root": "/home",
  "timestamp": 1740000000,
  "heft_version": "0.1.7",
  "file_count": 258423,
  "dir_count": 70713,
  "size_actual": 82899398656
}
```

Consumers can read the first 9 + N bytes to extract metadata without decompressing the tree payload.

### Rationale

- **Magic bytes** — allow `heft info` and other tools to identify the file format by content, not just extension.
- **Separate metadata** — cheap to read without decompressing the tree (useful for file browsers, info commands).
- **Zstd compression** — level 3 gives ~20:1 compression on the JSON tree with fast decompression.
- **Compact JSON** — `to_writer` (not `to_writer_pretty`) saves ~30% on pre-compression size.

### Round-tripping

`heft pack` builds the in-memory `TreeNode` tree from LMDB, serializes it as zstd-compressed JSON, and writes the `.heft` file. `heft unpack` reverses this: it deserializes the tree and reconstructs the LMDB entries by walking each directory node and encoding its children as `EntryRecord`s.
