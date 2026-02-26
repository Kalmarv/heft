use serde::{Deserialize, Serialize};

/// A node in the aggregated directory tree, ready for JSON export.
#[derive(Debug, Serialize, Deserialize)]
pub struct TreeNode {
    pub name: String,
    pub size_apparent: u64,
    pub size_actual: u64,
    pub mtime: u64,
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<TreeNode>>,
}
