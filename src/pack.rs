use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::db::HeftDb;
use crate::entry::{encode_many, EntryRecord, EntryType};
use crate::error::{HeftError, Result};
use crate::scanner::{format_count, format_size};
use crate::tree::{build_tree, TreeNode};

/// Magic bytes identifying a .heft file, plus format version.
const MAGIC: &[u8; 5] = b"HEFT\x01";

/// Metadata stored in the .heft file header (readable without decompression).
#[derive(Debug, Serialize, Deserialize)]
pub struct PackMeta {
    pub root: String,
    pub timestamp: u64,
    pub heft_version: String,
    pub file_count: u64,
    pub dir_count: u64,
    pub size_actual: u64,
}

/// Pack the database tree into a portable `.heft` file.
pub fn run_pack(
    db: &HeftDb,
    root_path: &str,
    output: &Path,
) -> Result<()> {
    let tree = build_tree(db, root_path)?;

    // Compute metadata from the tree.
    let (file_count, dir_count) = count_nodes(&tree);
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let meta = PackMeta {
        root: root_path.to_string(),
        timestamp,
        heft_version: env!("CARGO_PKG_VERSION").to_string(),
        file_count,
        dir_count,
        size_actual: tree.size_actual,
    };

    let mut file = File::create(output)?;

    // Write magic.
    file.write_all(MAGIC)?;

    // Write metadata length + metadata JSON.
    let meta_json = serde_json::to_vec(&meta)?;
    file.write_all(&(meta_json.len() as u32).to_le_bytes())?;
    file.write_all(&meta_json)?;

    // Write zstd-compressed tree JSON.
    let mut encoder = zstd::Encoder::new(&mut file, 3)?;
    serde_json::to_writer(&mut encoder, &tree)?;
    encoder.finish()?;

    let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);
    println!(
        "Packed {} ({} files, {} dirs, {}) → {} ({})",
        root_path,
        format_count(file_count),
        format_count(dir_count),
        format_size(tree.size_actual),
        output.display(),
        format_size(file_size),
    );

    Ok(())
}

/// Unpack a `.heft` file into the LMDB database.
pub fn run_unpack(
    input: &Path,
    db_path: &Path,
    map_size_gib: u64,
) -> Result<()> {
    let mut file = File::open(input)?;

    // Validate magic.
    let mut magic = [0u8; 5];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(HeftError::Other(format!(
            "not a .heft file (bad magic): {}",
            input.display()
        )));
    }

    // Read metadata.
    let mut len_buf = [0u8; 4];
    file.read_exact(&mut len_buf)?;
    let meta_len = u32::from_le_bytes(len_buf) as usize;
    let mut meta_buf = vec![0u8; meta_len];
    file.read_exact(&mut meta_buf)?;
    let meta: PackMeta = serde_json::from_slice(&meta_buf)?;

    // Decompress and deserialize tree.
    let decoder = zstd::Decoder::new(&mut file)?;
    let tree: TreeNode = serde_json::from_reader(decoder)?;

    // Write tree into LMDB.
    let db = HeftDb::open(db_path, map_size_gib)?;
    db.clear_subtree(&meta.root)?;

    let mut wtxn = db.write_txn()?;
    write_tree_to_db(&db, &mut wtxn, &meta.root, &tree)?;
    wtxn.commit()?;

    db.put_root(&meta.root)?;

    println!(
        "Unpacked {} → {} ({} files, {} dirs, {})",
        input.display(),
        db_path.display(),
        format_count(meta.file_count),
        format_count(meta.dir_count),
        format_size(meta.size_actual),
    );

    Ok(())
}

/// Read only the metadata header from a `.heft` file (no decompression).
pub fn read_info(input: &Path) -> Result<PackMeta> {
    let mut file = File::open(input)?;

    let mut magic = [0u8; 5];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(HeftError::Other(format!(
            "not a .heft file (bad magic): {}",
            input.display()
        )));
    }

    let mut len_buf = [0u8; 4];
    file.read_exact(&mut len_buf)?;
    let meta_len = u32::from_le_bytes(len_buf) as usize;
    let mut meta_buf = vec![0u8; meta_len];
    file.read_exact(&mut meta_buf)?;

    Ok(serde_json::from_slice(&meta_buf)?)
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Count files and directories in a tree (directories include the root).
fn count_nodes(node: &TreeNode) -> (u64, u64) {
    let mut files = 0u64;
    let mut dirs = 1u64; // count this node as a directory
    if let Some(children) = &node.children {
        for child in children {
            if child.children.is_some() {
                let (f, d) = count_nodes(child);
                files += f;
                dirs += d;
            } else {
                files += 1;
            }
        }
    }
    (files, dirs)
}

/// Recursively write a TreeNode into the LMDB database.
///
/// For each directory node, we collect its children as `EntryRecord`s and
/// write them under the directory's path key. Leaf nodes (files) are stored
/// as part of their parent's entry list, not as separate keys.
fn write_tree_to_db(
    db: &HeftDb,
    wtxn: &mut heed::RwTxn<'_>,
    dir_path: &str,
    node: &TreeNode,
) -> Result<()> {
    let children = match &node.children {
        Some(c) => c,
        None => return Ok(()),
    };

    let mut entries = Vec::with_capacity(children.len());
    for child in children {
        let is_dir = child.children.is_some();
        entries.push(EntryRecord {
            entry_type: if is_dir { EntryType::Dir } else { EntryType::File },
            mtime: child.mtime,
            size_apparent: if is_dir { 0 } else { child.size_apparent },
            size_actual: if is_dir { 0 } else { child.size_actual },
            name: child.name.clone(),
        });
    }

    let encoded = encode_many(&entries);
    db.put_dir_in_txn(wtxn, dir_path, &encoded)?;

    // Recurse into child directories.
    let trimmed = dir_path.trim_end_matches('/');
    for child in children {
        if child.children.is_some() {
            let child_path = format!("{}/{}", trimmed, child.name);
            write_tree_to_db(db, wtxn, &child_path, child)?;
        }
    }

    Ok(())
}
