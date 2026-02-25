use serde::Serialize;

use crate::db::DuxDb;
use crate::entry::EntryType;
use crate::error::Result;

/// A node in the aggregated directory tree, ready for JSON export.
#[derive(Debug, Serialize)]
pub struct TreeNode {
    pub name: String,
    pub size_apparent: u64,
    pub size_actual: u64,
    pub mtime: u64,
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<TreeNode>>,
}

/// Build the full in-memory tree from the database starting at `root_path`.
pub fn build_tree(db: &DuxDb, root_path: &str) -> Result<TreeNode> {
    build_node(db, root_path, path_basename(root_path))
}

fn build_node(db: &DuxDb, dir_path: &str, name: String) -> Result<TreeNode> {
    let entries = db.get_dir(dir_path)?.unwrap_or_default();

    let mut children = Vec::new();
    let mut size_apparent: u64 = 0;
    let mut size_actual: u64 = 0;
    let mut mtime: u64 = 0;
    let mut count: u64 = 0;

    for entry in &entries {
        if entry.entry_type == EntryType::Dir {
            // Recurse into subdirectory.
            let child_path = if dir_path.ends_with('/') {
                format!("{}{}", dir_path, entry.name)
            } else {
                format!("{}/{}", dir_path, entry.name)
            };
            let child_node = build_node(db, &child_path, entry.name.clone())?;
            size_apparent += child_node.size_apparent;
            size_actual += child_node.size_actual;
            mtime = mtime.max(child_node.mtime);
            count += child_node.count;
            children.push(child_node);
        } else {
            // Leaf entry (file, symlink, other).
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
    children.sort_by(|a, b| b.size_actual.cmp(&a.size_actual));

    // The directory itself counts as 1 + its descendants.
    count += 1;

    Ok(TreeNode {
        name,
        size_apparent,
        size_actual,
        mtime,
        count,
        children: if children.is_empty() {
            None
        } else {
            Some(children)
        },
    })
}

fn path_basename(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string())
}
