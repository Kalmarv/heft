# Hardlink Metadata in Index

**Date:** 2026-03-06
**Status:** Approved

## Problem

heft deduplicates hardlinks at scan time for the global size total, but discards
the inode metadata. Per-directory subtree sizes naively sum all entries, so
hardlinked files are double-counted. This produces inflated sizes for rsync
`--link-dest` backup trees, media libraries with cross-directory hardlinks, and
similar layouts.

## Design

Store hardlink metadata (`nlink`, `dev`, `ino`) in the index so that downstream
readers can implement their own dedup or filtering policies. heft remains
policy-free — it stores raw facts, readers decide what to do with them.

### Scope

**In scope (heft changes):**
- Add `nlink`, `dev`, `ino` fields to `EntryRecord`
- Encode/decode them in the binary entry format
- Expose them in `TreeNode` and the packed `.heft` format
- Only populate `dev`/`ino` when `nlink > 1` (zero-cost for non-hardlinked files)

**Out of scope (reader decisions):**
- Subtree size dedup logic (reader walks with `HashSet<(dev, ino)>`)
- "Show/hide hardlinks" UI toggle (Canopy concern)
- Any specific dedup policy (first-seen-wins, split, annotate, etc.)

### Entry format change

Current per-entry layout:

```
entry_type      (1 byte)
mtime           (LEB128)
size_apparent   (LEB128)
size_actual     (LEB128)
name_len        (LEB128)
name_bytes      (UTF-8)
```

New layout:

```
entry_type      (1 byte)
mtime           (LEB128)
size_apparent   (LEB128)
size_actual     (LEB128)
nlink           (LEB128)          <- NEW
[if nlink > 1]:
  dev           (LEB128)          <- NEW, conditional
  ino           (LEB128)          <- NEW, conditional
name_len        (LEB128)
name_bytes      (UTF-8)
```

### Size overhead

| Scenario | Extra bytes per entry |
|---|---|
| Non-hardlinked file (`nlink=1`) | 1 byte |
| Hardlinked file (`nlink>1`) | ~7-10 bytes (nlink + dev + ino) |
| Directory entry | 1 byte |

For a typical filesystem where <1% of files are hardlinked, index growth is ~1
byte per entry. For heavy hardlink trees (rsync `--link-dest` with millions of
links), growth is ~7-10 bytes per hardlinked entry.

### Performance impact

- **Scan time:** Zero additional syscalls. `nlink`, `dev`, `ino` are already
  read from the `stat` struct that `jwalk` populates. Only cost is LEB128
  encoding into an in-memory buffer (~nanoseconds).
- **Index size:** ~1 byte per entry in the common case. Negligible.
- **Query time:** No change to heft's tree aggregation. Readers that want dedup
  pay the cost of a `HashSet<(dev, ino)>` lookup per hardlinked entry.

### Backward compatibility

Old indexes lack `nlink`/`dev`/`ino`. Two options:

1. **Format version bump** — increment the format version, reject old indexes
   (user re-scans). Simple, clean.
2. **Graceful fallback** — detect old format by checking remaining bytes after
   `size_actual`. If missing, default to `nlink=1`.

Recommend option 1. A re-scan is fast and avoids format ambiguity.

### Reader dedup example

A reader wanting accurate subtree sizes would do:

```rust
let mut seen: HashSet<(u64, u64)> = HashSet::new();

fn walk(node: &TreeNode) -> u64 {
    let mut size = 0;
    for child in &node.children {
        if child.is_dir() {
            size += walk(child);
        } else if child.nlink > 1 {
            if seen.insert((child.dev, child.ino)) {
                size += child.size_actual;
            }
            // else: already counted, skip
        } else {
            size += child.size_actual;
        }
    }
    size
}
```

A "hide hardlinks" mode is even simpler — just skip entries where
`nlink > 1 && !seen.insert((dev, ino))`.

## Files to change

1. `crates/heft/src/entry.rs` — add fields, update encode/decode, update tests
2. `crates/heft/src/scanner.rs` — populate `nlink`/`dev`/`ino` on `EntryRecord`
3. `crates/heft/src/tree.rs` — expose fields in `TreeNode` and `Aggregate`
4. `crates/heft-format/src/tree.rs` — add fields to `TreeNode` struct
5. `crates/heft-format/src/pack.rs` — bump MAGIC version byte
6. `crates/heft/src/db.rs` — update `put_root` sentinel to include new fields

---

# Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Store hardlink metadata (nlink, dev, ino) in heft's index so downstream readers can deduplicate or filter hardlinked files.

**Architecture:** Add three fields to `EntryRecord`, conditionally encode `dev`/`ino` only when `nlink > 1`, propagate through `TreeNode` to the `.heft` pack format. Bump the pack format version. No dedup policy changes — readers decide.

**Tech Stack:** Rust, LEB128 encoding, serde/JSON, zstd-compressed `.heft` files

---

### Task 1: Add hardlink fields to EntryRecord and update encoding

**Files:**
- Modify: `crates/heft/src/entry.rs:25-37` (struct)
- Modify: `crates/heft/src/entry.rs:56-77` (encode_many)
- Modify: `crates/heft/src/entry.rs:79-109` (decode_many)

**Step 1: Update EntryRecord struct**

Add three new fields after `size_actual`:

```rust
/// A single filesystem entry stored in the database.
#[derive(Debug, Clone)]
pub struct EntryRecord {
    pub entry_type: EntryType,
    /// Modification time as seconds since Unix epoch.
    pub mtime: u64,
    /// Apparent (logical) size in bytes.
    pub size_apparent: u64,
    /// Actual (on-disk) size in bytes.
    pub size_actual: u64,
    /// Number of hard links (1 = no other links).
    pub nlink: u64,
    /// Device ID (only meaningful when nlink > 1).
    pub dev: u64,
    /// Inode number (only meaningful when nlink > 1).
    pub ino: u64,
    /// Filename (just the basename, not the full path).
    pub name: String,
}
```

**Step 2: Update encode_many**

Update the doc comment and encoding logic. After writing `size_actual`, write `nlink`. If `nlink > 1`, also write `dev` and `ino`:

```rust
/// Encode a slice of entries into a compact binary format.
///
/// Layout per entry:
///   entry_type    (1 byte)
///   mtime         (LEB128)
///   size_apparent (LEB128)
///   size_actual   (LEB128)
///   nlink         (LEB128)
///   [if nlink > 1]:
///     dev         (LEB128)
///     ino         (LEB128)
///   name_len      (LEB128)
///   name_bytes    (UTF-8, name_len bytes)
pub fn encode_many(entries: &[EntryRecord]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(entries.len() * 32);
    for e in entries {
        buf.push(e.entry_type as u8);
        write_varint(&mut buf, e.mtime);
        write_varint(&mut buf, e.size_apparent);
        write_varint(&mut buf, e.size_actual);
        write_varint(&mut buf, e.nlink);
        if e.nlink > 1 {
            write_varint(&mut buf, e.dev);
            write_varint(&mut buf, e.ino);
        }
        let name_bytes = e.name.as_bytes();
        write_varint(&mut buf, name_bytes.len() as u64);
        buf.extend_from_slice(name_bytes);
    }
    buf
}
```

**Step 3: Update decode_many**

After reading `size_actual`, read `nlink`. If `nlink > 1`, read `dev` and `ino`, else default them to 0:

```rust
pub fn decode_many(data: &[u8]) -> Result<Vec<EntryRecord>> {
    let mut entries = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let entry_type = EntryType::from_u8(data[pos])?;
        pos += 1;
        let mtime = read_varint(data, &mut pos)?;
        let size_apparent = read_varint(data, &mut pos)?;
        let size_actual = read_varint(data, &mut pos)?;
        let nlink = read_varint(data, &mut pos)?;
        let (dev, ino) = if nlink > 1 {
            (read_varint(data, &mut pos)?, read_varint(data, &mut pos)?)
        } else {
            (0, 0)
        };
        let name_len = read_varint(data, &mut pos)? as usize;
        if pos + name_len > data.len() {
            return Err(HeftError::Encoding("truncated name".into()));
        }
        let name_bytes = &data[pos..pos + name_len];
        let name = match std::str::from_utf8(name_bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => String::from_utf8_lossy(name_bytes).into_owned(),
        };
        pos += name_len;
        entries.push(EntryRecord {
            entry_type,
            mtime,
            size_apparent,
            size_actual,
            nlink,
            dev,
            ino,
            name,
        });
    }
    Ok(entries)
}
```

**Step 4: Update all tests in entry.rs**

Update `assert_entry` to check the new fields, and update every test to include `nlink`, `dev`, `ino` in `EntryRecord` constructors:

```rust
fn assert_entry(
    got: &EntryRecord,
    entry_type: EntryType,
    mtime: u64,
    size_apparent: u64,
    size_actual: u64,
    nlink: u64,
    dev: u64,
    ino: u64,
    name: &str,
) {
    assert_eq!(got.entry_type, entry_type, "entry_type mismatch for '{name}'");
    assert_eq!(got.mtime, mtime, "mtime mismatch for '{name}'");
    assert_eq!(got.size_apparent, size_apparent, "size_apparent mismatch for '{name}'");
    assert_eq!(got.size_actual, size_actual, "size_actual mismatch for '{name}'");
    assert_eq!(got.nlink, nlink, "nlink mismatch for '{name}'");
    assert_eq!(got.dev, dev, "dev mismatch for '{name}'");
    assert_eq!(got.ino, ino, "ino mismatch for '{name}'");
    assert_eq!(got.name, name, "name mismatch");
}
```

All existing test `EntryRecord` literals need `nlink: 1, dev: 0, ino: 0` added. Add one new test for hardlinked entry round-trip:

```rust
#[test]
fn round_trip_hardlink() {
    let entries = vec![EntryRecord {
        entry_type: EntryType::File,
        mtime: 1700000000,
        size_apparent: 4096,
        size_actual: 8192,
        nlink: 3,
        dev: 66306,
        ino: 12345678,
        name: "shared.dat".into(),
    }];
    let encoded = encode_many(&entries);
    let decoded = decode_many(&encoded).unwrap();
    assert_eq!(decoded.len(), 1);
    assert_entry(
        &decoded[0], EntryType::File, 1700000000, 4096, 8192,
        3, 66306, 12345678, "shared.dat",
    );
}
```

**Step 5: Build and run tests**

Run: `docker exec -it myapp-dev cargo test -p heft --lib entry`
Expected: All tests pass including the new `round_trip_hardlink` test.

**Step 6: Commit**

```bash
git add crates/heft/src/entry.rs
git commit -m "Add nlink/dev/ino fields to EntryRecord with conditional encoding"
```

---

### Task 2: Populate hardlink fields in scanner

**Files:**
- Modify: `crates/heft/src/scanner.rs:246-252` (EntryRecord construction)

**Step 1: Capture nlink/dev/ino from metadata and pass to EntryRecord**

At `scanner.rs:246`, update the `EntryRecord` construction. The metadata is already available from line 181. Read `nlink`, and conditionally read `dev`/`ino`:

```rust
let nlink = metadata.nlink();
let (dev, ino) = if nlink > 1 {
    (metadata.dev(), metadata.ino())
} else {
    (0, 0)
};

entries.push(EntryRecord {
    entry_type,
    mtime,
    size_apparent,
    size_actual,
    nlink,
    dev,
    ino,
    name,
});
```

Place the `nlink`/`dev`/`ino` extraction just before the `entries.push(...)` call (after line 244, before line 246).

**Step 2: Update put_root sentinel in db.rs**

At `db.rs:56-62`, the `put_root` function creates an `EntryRecord`. Add the new fields with default values:

```rust
let sentinel = EntryRecord {
    entry_type: EntryType::Dir,
    mtime: 0,
    size_apparent: 0,
    size_actual: 0,
    nlink: 1,
    dev: 0,
    ino: 0,
    name: root_path.to_string(),
};
```

**Step 3: Build**

Run: `docker exec -it myapp-dev cargo build -p heft`
Expected: Compiles with no errors.

**Step 4: Commit**

```bash
git add crates/heft/src/scanner.rs crates/heft/src/db.rs
git commit -m "Populate nlink/dev/ino from filesystem metadata during scan"
```

---

### Task 3: Add hardlink fields to heft-format TreeNode

**Files:**
- Modify: `crates/heft-format/src/tree.rs:4-13` (TreeNode struct)

**Step 1: Add nlink, dev, ino to TreeNode**

Add the three fields with `serde` skip-if-default to keep JSON compact for non-hardlinked files:

```rust
/// A node in the aggregated directory tree, ready for JSON export.
#[derive(Debug, Serialize, Deserialize)]
pub struct TreeNode {
    pub name: String,
    pub size_apparent: u64,
    pub size_actual: u64,
    pub mtime: u64,
    pub count: u64,
    #[serde(default, skip_serializing_if = "is_one")]
    pub nlink: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub dev: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub ino: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<TreeNode>>,
}

fn is_zero(v: &u64) -> bool { *v == 0 }
fn is_one(v: &u64) -> bool { *v <= 1 }
```

Note: `serde(default)` ensures old `.heft` files without these fields deserialize with `nlink=0`, `dev=0`, `ino=0`. The `is_one` skip for `nlink` avoids serializing `"nlink": 1` for the vast majority of entries.

**Step 2: Build**

Run: `docker exec -it myapp-dev cargo build -p heft-format`
Expected: Compiles. (Will have errors in heft crate until Task 4.)

**Step 3: Commit**

```bash
git add crates/heft-format/src/tree.rs
git commit -m "Add nlink/dev/ino fields to heft-format TreeNode"
```

---

### Task 4: Propagate hardlink fields in tree builder

**Files:**
- Modify: `crates/heft/src/tree.rs:48-61` (file branch of build_node)
- Modify: `crates/heft/src/tree.rs:69-76` (directory TreeNode in build_node)

**Step 1: Update build_node to pass hardlink fields for file entries**

In `tree.rs:53-60`, the file-entry `TreeNode` construction needs the new fields:

```rust
children.push(TreeNode {
    name: entry.name.clone(),
    size_apparent: entry.size_apparent,
    size_actual: entry.size_actual,
    mtime: entry.mtime,
    count: 1,
    nlink: entry.nlink,
    dev: entry.dev,
    ino: entry.ino,
    children: None,
});
```

For directory nodes (`tree.rs:69-76`), directories themselves aren't hardlinked, so use defaults:

```rust
Ok(TreeNode {
    name,
    size_apparent,
    size_actual,
    mtime,
    count,
    nlink: 1,
    dev: 0,
    ino: 0,
    children: Some(children),
})
```

**Step 2: Build and run tests**

Run: `docker exec -it myapp-dev cargo test -p heft`
Expected: All tests pass, no compile errors.

**Step 3: Commit**

```bash
git add crates/heft/src/tree.rs
git commit -m "Propagate nlink/dev/ino through tree builder to TreeNode"
```

---

### Task 5: Bump .heft format version

**Files:**
- Modify: `crates/heft-format/src/pack.rs:13` (MAGIC constant)
- Modify: `crates/heft-format/src/pack.rs:80-88` (load_tree magic check)

**Step 1: Bump MAGIC version byte from \x01 to \x02**

```rust
const MAGIC: &[u8; 5] = b"HEFT\x02";
```

This ensures old `.heft` files (which lack hardlink data in their tree JSON) are rejected with a clear error rather than silently deserializing with missing fields.

**Step 2: Build**

Run: `docker exec -it myapp-dev cargo build -p heft`
Expected: Compiles cleanly.

**Step 3: Commit**

```bash
git add crates/heft-format/src/pack.rs
git commit -m "Bump .heft format version to 2 for hardlink metadata"
```

---

### Task 6: End-to-end verification

**Step 1: Build the release binary**

Run: `docker exec -it myapp-dev cargo build -p heft --release`

**Step 2: Run a scan on a test directory with hardlinks**

Run:
```bash
docker exec -it myapp-dev bash -c '
  mkdir -p /tmp/heft-test/a /tmp/heft-test/b
  echo "shared content" > /tmp/heft-test/a/file.txt
  ln /tmp/heft-test/a/file.txt /tmp/heft-test/b/file.txt
  echo "unique" > /tmp/heft-test/a/unique.txt
  target/release/heft scan /tmp/heft-test -o /tmp/heft-test.heft
  target/release/heft ls /tmp/heft-test.heft
  target/release/heft info /tmp/heft-test.heft
  rm -rf /tmp/heft-test /tmp/heft-test.heft
'
```

Expected: Scan completes, `ls` shows directory listing, `info` shows metadata. No panics or errors.

**Step 3: Verify hardlink data is in the .heft JSON**

Run:
```bash
docker exec -it myapp-dev bash -c '
  mkdir -p /tmp/heft-test2/a /tmp/heft-test2/b
  echo "shared" > /tmp/heft-test2/a/file.txt
  ln /tmp/heft-test2/a/file.txt /tmp/heft-test2/b/file.txt
  target/release/heft scan /tmp/heft-test2 -o /tmp/test2.heft
  # Extract and inspect the JSON tree (skip 9-byte header + metadata)
  python3 -c "
import struct, json, zstd, sys
f = open(\"/tmp/test2.heft\", \"rb\")
f.read(5)  # magic
ml = struct.unpack(\"<I\", f.read(4))[0]
f.read(ml)  # metadata
tree = json.loads(zstd.decompress(f.read()))
print(json.dumps(tree, indent=2))
" 2>/dev/null || echo "(python3/zstd not available — skip JSON inspection)"
  rm -rf /tmp/heft-test2 /tmp/test2.heft
'
```

Expected: The JSON tree shows `"nlink": 2`, `"dev": ...`, `"ino": ...` for `file.txt` entries. Non-hardlinked files should NOT have nlink/dev/ino fields (they're skipped by serde).

**Step 4: Run the full test suite**

Run: `docker exec -it myapp-dev cargo test --workspace`
Expected: All tests pass.

**Step 5: Commit (if any fixups were needed)**

```bash
git add -A
git commit -m "Fix any issues found during e2e verification"
```
