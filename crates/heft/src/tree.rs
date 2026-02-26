use heft_format::TreeNode;

use crate::db::HeftDb;
use crate::entry::EntryType;
use crate::error::Result;

/// Lightweight aggregate — only computes sizes/mtime/count without building the
/// full TreeNode graph. Used by `ls` to avoid allocating child node vectors.
pub struct Aggregate {
    pub size_apparent: u64,
    pub size_actual: u64,
    pub mtime: u64,
    pub count: u64,
    pub file_count: u64,
    pub dir_count: u64,
}

/// Build the full in-memory tree from the database starting at `root_path`.
/// Opens a single read transaction for the entire traversal.
pub fn build_tree(db: &HeftDb, root_path: &str) -> Result<TreeNode> {
    let rtxn = db.read_txn()?;
    build_node(db, &rtxn, root_path, path_basename(root_path))
}

fn build_node(
    db: &HeftDb,
    rtxn: &heed::RoTxn<'_>,
    dir_path: &str,
    name: String,
) -> Result<TreeNode> {
    let entries = db.get_dir_in_txn(rtxn, dir_path)?.unwrap_or_default();

    let mut children = Vec::with_capacity(entries.len());
    let mut size_apparent: u64 = 0;
    let mut size_actual: u64 = 0;
    let mut mtime: u64 = 0;
    let mut count: u64 = 0;

    for entry in &entries {
        if entry.entry_type == EntryType::Dir {
            let child_path = format!("{}/{}", dir_path.trim_end_matches('/'), entry.name);
            let child_node = build_node(db, rtxn, &child_path, entry.name.clone())?;
            size_apparent += child_node.size_apparent;
            size_actual += child_node.size_actual;
            mtime = mtime.max(child_node.mtime);
            count += child_node.count;
            children.push(child_node);
        } else {
            size_apparent += entry.size_apparent;
            size_actual += entry.size_actual;
            mtime = mtime.max(entry.mtime);
            count += 1;
            children.push(TreeNode {
                name: entry.name.clone(),
                size_apparent: entry.size_apparent,
                size_actual: entry.size_actual,
                mtime: entry.mtime,
                count: 1,
                children: None,
            });
        }
    }

    // Sort children by size_actual descending.
    children.sort_unstable_by(|a, b| b.size_actual.cmp(&a.size_actual));

    count += 1;

    Ok(TreeNode {
        name,
        size_apparent,
        size_actual,
        mtime,
        count,
        children: Some(children),
    })
}

/// Compute aggregate sizes for a directory without building full TreeNode objects.
/// Much cheaper than build_tree when you only need the top-level numbers.
pub fn aggregate_sizes(db: &HeftDb, rtxn: &heed::RoTxn<'_>, dir_path: &str) -> Result<Aggregate> {
    let entries = db.get_dir_in_txn(rtxn, dir_path)?.unwrap_or_default();

    let mut size_apparent: u64 = 0;
    let mut size_actual: u64 = 0;
    let mut mtime: u64 = 0;
    let mut count: u64 = 0;
    let mut file_count: u64 = 0;
    let mut dir_count: u64 = 0;

    for entry in &entries {
        if entry.entry_type == EntryType::Dir {
            let child_path = format!("{}/{}", dir_path.trim_end_matches('/'), entry.name);
            let child = aggregate_sizes(db, rtxn, &child_path)?;
            size_apparent += child.size_apparent;
            size_actual += child.size_actual;
            mtime = mtime.max(child.mtime);
            count += child.count;
            file_count += child.file_count;
            dir_count += child.dir_count;
        } else {
            size_apparent += entry.size_apparent;
            size_actual += entry.size_actual;
            mtime = mtime.max(entry.mtime);
            count += 1;
            file_count += 1;
        }
    }

    count += 1;
    dir_count += 1;
    Ok(Aggregate {
        size_apparent,
        size_actual,
        mtime,
        count,
        file_count,
        dir_count,
    })
}

fn path_basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}
