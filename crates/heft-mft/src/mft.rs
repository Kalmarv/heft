use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek};

use ntfs::structured_values::NtfsFileNamespace;
use ntfs::{Ntfs, NtfsFile, NtfsFileFlags};

use heft_format::{HeftError, TreeNode};

use crate::volume;

/// 100-nanosecond intervals between 1601-01-01 (NTFS epoch) and 1970-01-01 (Unix epoch).
const FILETIME_EPOCH_OFFSET: u64 = 116_444_736_000_000_000;
const INTERVALS_PER_SECOND: u64 = 10_000_000;

/// NTFS root directory record number.
const ROOT_RECORD: u64 = 5;

struct MftEntry {
    parent_record_number: u64,
    name: String,
    is_directory: bool,
    size_apparent: u64,
    size_actual: u64,
    mtime: u64,
}

/// Convert an NTFS timestamp (100ns intervals since 1601) to Unix epoch seconds.
fn filetime_to_unix(nt_timestamp: u64) -> u64 {
    if nt_timestamp <= FILETIME_EPOCH_OFFSET {
        return 0;
    }
    (nt_timestamp - FILETIME_EPOCH_OFFSET) / INTERVALS_PER_SECOND
}

fn ntfs_err(e: ntfs::NtfsError) -> HeftError {
    HeftError::Other(format!("NTFS error: {e}"))
}

/// Get the best filename from a file record.
///
/// Prefers Win32 > Win32AndDos > Posix. Skips Dos-only names (they always
/// duplicate a Win32 entry). Returns (name, parent_record_number, data_size,
/// allocated_size).
fn best_file_name<T: Read + Seek>(
    file: &NtfsFile,
    fs: &mut T,
) -> Option<(String, u64, u64, u64)> {
    let priorities = [
        NtfsFileNamespace::Win32,
        NtfsFileNamespace::Win32AndDos,
        NtfsFileNamespace::Posix,
    ];

    for &ns in &priorities {
        if let Some(Ok(name_attr)) = file.name(fs, Some(ns), None) {
            return Some((
                name_attr.name().to_string_lossy(),
                name_attr.parent_directory_reference().file_record_number(),
                name_attr.data_size(),
                name_attr.allocated_size(),
            ));
        }
    }

    // Fallback: accept any namespace except Dos-only.
    if let Some(Ok(name_attr)) = file.name(fs, None, None) {
        if name_attr.namespace() != NtfsFileNamespace::Dos {
            return Some((
                name_attr.name().to_string_lossy(),
                name_attr.parent_directory_reference().file_record_number(),
                name_attr.data_size(),
                name_attr.allocated_size(),
            ));
        }
    }

    None
}

/// Pass 1: sequentially read all MFT records into a flat HashMap.
fn read_all_records<T: Read + Seek>(
    ntfs: &Ntfs,
    fs: &mut T,
    record_count: u64,
    verbose: bool,
) -> Result<HashMap<u64, MftEntry>, HeftError> {
    let mut entries = HashMap::with_capacity(record_count as usize);

    for i in 0..record_count {
        if verbose && i % 100_000 == 0 && i > 0 {
            eprintln!("  reading MFT record {i}/{record_count}...");
        }

        let file = match ntfs.file(fs, i) {
            Ok(f) => f,
            Err(_) => continue, // corrupt or sparse record
        };

        if !file.flags().contains(NtfsFileFlags::IN_USE) {
            continue;
        }

        let is_directory = file.is_directory();

        // Get mtime from $STANDARD_INFORMATION.
        let mtime = match file.info() {
            Ok(info) => filetime_to_unix(info.modification_time().nt_timestamp()),
            Err(_) => 0,
        };

        let (name, parent_ref, data_size, alloc_size) = match best_file_name(&file, fs) {
            Some(v) => v,
            None => continue,
        };

        let (size_apparent, size_actual) = if is_directory {
            (0, 0) // aggregated from children in Pass 2
        } else {
            (data_size, alloc_size)
        };

        entries.insert(
            i,
            MftEntry {
                parent_record_number: parent_ref,
                name,
                is_directory,
                size_apparent,
                size_actual,
                mtime,
            },
        );
    }

    Ok(entries)
}

/// Pass 2: build a TreeNode tree from flat MFT entries.
fn build_tree_from_entries(
    entries: &HashMap<u64, MftEntry>,
    root_record: u64,
    root_name: String,
) -> TreeNode {
    // Build parent → children mapping.
    let mut children_map: HashMap<u64, Vec<u64>> = HashMap::new();
    for (&rec_num, entry) in entries {
        if rec_num != root_record {
            children_map
                .entry(entry.parent_record_number)
                .or_default()
                .push(rec_num);
        }
    }

    let mut visited = HashSet::new();
    build_node(
        root_record,
        &root_name,
        entries,
        &children_map,
        &mut visited,
    )
}

fn build_node(
    record: u64,
    name: &str,
    entries: &HashMap<u64, MftEntry>,
    children_map: &HashMap<u64, Vec<u64>>,
    visited: &mut HashSet<u64>,
) -> TreeNode {
    // Cycle detection (shouldn't happen on healthy NTFS).
    if !visited.insert(record) {
        return TreeNode {
            name: name.to_string(),
            size_apparent: 0,
            size_actual: 0,
            mtime: 0,
            count: 1,
            children: Some(Vec::new()),
        };
    }

    let entry = entries.get(&record);
    let base_mtime = entry.map_or(0, |e| e.mtime);

    match children_map.get(&record) {
        Some(child_recs) => {
            let mut child_nodes: Vec<TreeNode> = Vec::new();
            let mut total_apparent: u64 = 0;
            let mut total_actual: u64 = 0;
            let mut total_count: u64 = 0;
            let mut max_mtime: u64 = base_mtime;

            for &child_rec in child_recs {
                if let Some(child_entry) = entries.get(&child_rec) {
                    if child_entry.is_directory {
                        let child_node = build_node(
                            child_rec,
                            &child_entry.name,
                            entries,
                            children_map,
                            visited,
                        );
                        total_apparent += child_node.size_apparent;
                        total_actual += child_node.size_actual;
                        total_count += child_node.count;
                        max_mtime = max_mtime.max(child_node.mtime);
                        child_nodes.push(child_node);
                    } else {
                        total_apparent += child_entry.size_apparent;
                        total_actual += child_entry.size_actual;
                        total_count += 1;
                        max_mtime = max_mtime.max(child_entry.mtime);
                        child_nodes.push(TreeNode {
                            name: child_entry.name.clone(),
                            size_apparent: child_entry.size_apparent,
                            size_actual: child_entry.size_actual,
                            mtime: child_entry.mtime,
                            count: 1,
                            children: None,
                        });
                    }
                }
            }

            // Sort children by size_actual descending (matches heft convention).
            child_nodes.sort_by(|a, b| b.size_actual.cmp(&a.size_actual));

            total_count += 1; // count the directory itself

            TreeNode {
                name: name.to_string(),
                size_apparent: total_apparent,
                size_actual: total_actual,
                mtime: max_mtime,
                count: total_count,
                children: Some(child_nodes),
            }
        }

        // Directory with no children (or leaf entry reached via fallback).
        None => {
            if entry.map_or(true, |e| e.is_directory) {
                TreeNode {
                    name: name.to_string(),
                    size_apparent: 0,
                    size_actual: 0,
                    mtime: base_mtime,
                    count: 1,
                    children: Some(Vec::new()),
                }
            } else {
                let e = entry.unwrap();
                TreeNode {
                    name: name.to_string(),
                    size_apparent: e.size_apparent,
                    size_actual: e.size_actual,
                    mtime: e.mtime,
                    count: 1,
                    children: None,
                }
            }
        }
    }
}

/// Scan a volume's MFT and return a TreeNode plus the root label (e.g. "C:").
pub fn scan_volume(volume_spec: &str, verbose: bool) -> Result<(TreeNode, String), HeftError> {
    let mut reader = volume::open_volume(volume_spec)?;
    let ntfs = Ntfs::new(&mut reader).map_err(ntfs_err)?;

    // Get MFT record count from the $MFT file's $DATA attribute length.
    let record_count = {
        let mft_file = ntfs.file(&mut reader, 0).map_err(ntfs_err)?;
        let data_item = mft_file
            .data(&mut reader, "")
            .ok_or_else(|| HeftError::Other("MFT has no $DATA attribute".to_string()))?
            .map_err(ntfs_err)?;
        let data_attr = data_item.to_attribute().map_err(ntfs_err)?;
        data_attr.value_length() / ntfs.file_record_size() as u64
    };

    eprintln!("MFT contains {record_count} records");

    // Pass 1: read all records.
    let entries = read_all_records(&ntfs, &mut reader, record_count, verbose)?;
    eprintln!("Pass 1 complete: {} live entries", entries.len());

    // Pass 2: build tree from root (record 5).
    let letter = volume_spec
        .trim_end_matches([':', '\\', '/'])
        .chars()
        .next()
        .unwrap_or('C')
        .to_ascii_uppercase();
    let root_name = format!("{letter}:");
    let tree = build_tree_from_entries(&entries, ROOT_RECORD, root_name.clone());
    eprintln!("Pass 2 complete: tree built with {} items", tree.count);

    Ok((tree, root_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filetime_to_unix() {
        // 2021-01-01T00:00:00Z → NT timestamp 132539328000000000
        let nt = 132_539_328_000_000_000u64;
        let unix = filetime_to_unix(nt);
        assert_eq!(unix, 1_609_459_200); // 2021-01-01 00:00:00 UTC

        // 2013-01-05T18:15:00Z → NT timestamp 130018833000000000
        let nt2 = 130_018_833_000_000_000u64;
        let unix2 = filetime_to_unix(nt2);
        assert_eq!(unix2, 1_357_409_700);

        // Before Unix epoch → 0
        assert_eq!(filetime_to_unix(0), 0);
        assert_eq!(filetime_to_unix(FILETIME_EPOCH_OFFSET), 0);

        // Just after Unix epoch
        let one_sec_after = FILETIME_EPOCH_OFFSET + INTERVALS_PER_SECOND;
        assert_eq!(filetime_to_unix(one_sec_after), 1);
    }

    #[test]
    fn test_build_tree_from_entries() {
        // Simulate a small NTFS with root (5) containing a dir (100) and a file (101).
        // Dir 100 contains files 200 and 201.
        let mut entries = HashMap::new();

        // Root directory (record 5)
        entries.insert(
            5,
            MftEntry {
                parent_record_number: 5, // root is its own parent
                name: "C:".to_string(),
                is_directory: true,
                size_apparent: 0,
                size_actual: 0,
                mtime: 1000,
            },
        );

        // Subdirectory (record 100)
        entries.insert(
            100,
            MftEntry {
                parent_record_number: 5,
                name: "Users".to_string(),
                is_directory: true,
                size_apparent: 0,
                size_actual: 0,
                mtime: 2000,
            },
        );

        // File in root (record 101)
        entries.insert(
            101,
            MftEntry {
                parent_record_number: 5,
                name: "pagefile.sys".to_string(),
                is_directory: false,
                size_apparent: 4096,
                size_actual: 4096,
                mtime: 1500,
            },
        );

        // File in Users (record 200)
        entries.insert(
            200,
            MftEntry {
                parent_record_number: 100,
                name: "doc.txt".to_string(),
                is_directory: false,
                size_apparent: 100,
                size_actual: 512,
                mtime: 3000,
            },
        );

        // File in Users (record 201)
        entries.insert(
            201,
            MftEntry {
                parent_record_number: 100,
                name: "photo.jpg".to_string(),
                is_directory: false,
                size_apparent: 2048,
                size_actual: 4096,
                mtime: 2500,
            },
        );

        let tree = build_tree_from_entries(&entries, 5, "C:".to_string());

        // Root aggregation.
        assert_eq!(tree.name, "C:");
        assert_eq!(tree.size_apparent, 4096 + 100 + 2048); // all files
        assert_eq!(tree.size_actual, 4096 + 512 + 4096); // all files on disk
        assert_eq!(tree.count, 5); // 2 dirs + 3 files
        assert_eq!(tree.mtime, 3000); // max across all descendants

        let children = tree.children.as_ref().unwrap();
        assert_eq!(children.len(), 2);

        // Children are sorted by size_actual descending.
        // Users dir: size_actual = 512 + 4096 = 4608
        // pagefile.sys: size_actual = 4096
        assert_eq!(children[0].name, "Users");
        assert_eq!(children[0].size_actual, 512 + 4096);
        assert_eq!(children[0].count, 3); // dir itself + 2 files

        assert_eq!(children[1].name, "pagefile.sys");
        assert_eq!(children[1].size_actual, 4096);
        assert_eq!(children[1].count, 1);

        // Users children sorted by size_actual desc.
        let users_children = children[0].children.as_ref().unwrap();
        assert_eq!(users_children[0].name, "photo.jpg");
        assert_eq!(users_children[1].name, "doc.txt");
    }

    #[test]
    fn test_build_tree_empty_root() {
        let mut entries = HashMap::new();
        entries.insert(
            5,
            MftEntry {
                parent_record_number: 5,
                name: "D:".to_string(),
                is_directory: true,
                size_apparent: 0,
                size_actual: 0,
                mtime: 100,
            },
        );

        let tree = build_tree_from_entries(&entries, 5, "D:".to_string());
        assert_eq!(tree.name, "D:");
        assert_eq!(tree.count, 1);
        assert_eq!(tree.size_actual, 0);
        assert!(tree.children.as_ref().unwrap().is_empty());
    }

    #[test]
    fn test_build_tree_orphans_excluded() {
        // Orphaned file: parent 999 doesn't exist.
        let mut entries = HashMap::new();
        entries.insert(
            5,
            MftEntry {
                parent_record_number: 5,
                name: "C:".to_string(),
                is_directory: true,
                size_apparent: 0,
                size_actual: 0,
                mtime: 0,
            },
        );
        entries.insert(
            50,
            MftEntry {
                parent_record_number: 999, // orphan
                name: "lost.dat".to_string(),
                is_directory: false,
                size_apparent: 1000,
                size_actual: 1000,
                mtime: 0,
            },
        );

        let tree = build_tree_from_entries(&entries, 5, "C:".to_string());
        assert_eq!(tree.count, 1); // only root
        assert_eq!(tree.size_actual, 0); // orphan not included
    }
}
