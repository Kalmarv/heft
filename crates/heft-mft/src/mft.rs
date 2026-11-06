use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Read, Seek, Write};
use std::time::{Duration, Instant};

use ntfs::{Ntfs, NtfsReadSeek};

use heft_format::{HeftError, TreeNode};

use crate::volume;

/// 100-nanosecond intervals between 1601-01-01 (NTFS epoch) and 1970-01-01 (Unix epoch).
const FILETIME_EPOCH_OFFSET: u64 = 116_444_736_000_000_000;
const INTERVALS_PER_SECOND: u64 = 10_000_000;

/// NTFS root directory record number.
const ROOT_RECORD: u64 = 5;

/// NTFS attribute type IDs.
const ATTR_STANDARD_INFORMATION: u32 = 0x10;
const ATTR_FILE_NAME: u32 = 0x30;
const ATTR_DATA: u32 = 0x80;
const ATTR_END: u32 = 0xFFFF_FFFF;

/// $FILE_NAME namespace values.
const NS_POSIX: u8 = 0;
const NS_WIN32: u8 = 1;
const NS_DOS: u8 = 2;
const NS_WIN32_AND_DOS: u8 = 3;

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

// ── Fast MFT record parsing ────────────────────────────────────────────

/// Apply the NTFS update sequence array (USA) fixup to a raw record buffer.
///
/// Each MFT record spans multiple 512-byte sectors.  NTFS stores a check value
/// in the last two bytes of each sector and keeps the original bytes in the
/// USA.  We need to reverse this before parsing attributes.
fn apply_fixup(record: &mut [u8]) -> bool {
    if record.len() < 8 {
        return false;
    }
    let usa_ofs = u16::from_le_bytes([record[0x04], record[0x05]]) as usize;
    let usa_cnt = u16::from_le_bytes([record[0x06], record[0x07]]) as usize;

    if usa_cnt < 2 || usa_ofs + usa_cnt * 2 > record.len() {
        return false;
    }

    let usn = u16::from_le_bytes([record[usa_ofs], record[usa_ofs + 1]]);

    for i in 1..usa_cnt {
        let sector_end = i * 512;
        if sector_end > record.len() {
            break;
        }
        let pos = sector_end - 2;
        let stored = u16::from_le_bytes([record[pos], record[pos + 1]]);
        if stored != usn {
            return false;
        }
        record[pos] = record[usa_ofs + i * 2];
        record[pos + 1] = record[usa_ofs + i * 2 + 1];
    }
    true
}

/// Filename candidate extracted from a $FILE_NAME attribute.
struct FileNameInfo {
    name: String,
    parent_record: u64,
    namespace: u8,
}

/// Namespace priority: Win32 > Win32AndDos > Posix.  DOS-only is skipped entirely.
fn ns_priority(ns: u8) -> u8 {
    match ns {
        NS_WIN32 => 3,
        NS_WIN32_AND_DOS => 2,
        NS_POSIX => 1,
        _ => 0,
    }
}

/// Result of parsing a single MFT record.
enum ParsedRecord {
    /// Base record — a complete file/directory entry.
    Base(MftEntry),
    /// Extension record — carries $DATA sizes for its base record.
    Extension {
        base_record: u64,
        data_size: u64,
        alloc_size: u64,
    },
}

/// Parse a single MFT record from a byte slice.
fn parse_mft_record(record_buf: &[u8]) -> Option<ParsedRecord> {
    if record_buf.len() < 0x38 {
        return None;
    }

    // Check "FILE" signature.
    if &record_buf[0..4] != b"FILE" {
        return None;
    }

    // Apply USA fixup on a copy.
    let mut rec = record_buf.to_vec();
    if !apply_fixup(&mut rec) {
        return None;
    }

    // Flags: bit 0 = IN_USE, bit 1 = IS_DIRECTORY.
    let flags = u16::from_le_bytes([rec[0x16], rec[0x17]]);
    if flags & 0x01 == 0 {
        return None; // not in use
    }
    let is_directory = flags & 0x02 != 0;

    // Determine if this is a base or extension record.
    let base_ref_lo = u32::from_le_bytes(rec[0x20..0x24].try_into().unwrap()) as u64;
    let base_ref_hi = u16::from_le_bytes([rec[0x24], rec[0x25]]) as u64;
    let base_record = base_ref_lo | (base_ref_hi << 32);
    let is_extension = base_record != 0;

    let first_attr = u16::from_le_bytes([rec[0x14], rec[0x15]]) as usize;
    let used_size = u32::from_le_bytes(rec[0x18..0x1C].try_into().unwrap()) as usize;
    let limit = used_size.min(rec.len());

    let mut mtime: u64 = 0;
    let mut best: Option<FileNameInfo> = None;
    let mut data_size: u64 = 0;
    let mut alloc_size: u64 = 0;

    let mut pos = first_attr;
    while pos + 8 <= limit {
        let attr_type = u32::from_le_bytes(rec[pos..pos + 4].try_into().unwrap());
        if attr_type == ATTR_END {
            break;
        }
        let attr_len = u32::from_le_bytes(rec[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if attr_len < 8 || pos + attr_len > limit {
            break;
        }

        let non_resident = rec[pos + 8];
        let name_len = rec[pos + 9] as usize;

        match attr_type {
            ATTR_STANDARD_INFORMATION if non_resident == 0 && !is_extension => {
                if pos + 0x16 <= limit {
                    let val_ofs = u16::from_le_bytes([rec[pos + 0x14], rec[pos + 0x15]]) as usize;
                    let val_pos = pos + val_ofs;
                    if val_pos + 16 <= pos + attr_len {
                        let nt_mtime =
                            u64::from_le_bytes(rec[val_pos + 8..val_pos + 16].try_into().unwrap());
                        mtime = filetime_to_unix(nt_mtime);
                    }
                }
            }
            ATTR_FILE_NAME if non_resident == 0 && !is_extension => {
                if pos + 0x16 <= limit {
                    let val_ofs = u16::from_le_bytes([rec[pos + 0x14], rec[pos + 0x15]]) as usize;
                    let val_pos = pos + val_ofs;
                    let val_len =
                        u32::from_le_bytes(rec[pos + 0x10..pos + 0x14].try_into().unwrap())
                            as usize;

                    if val_len >= 0x42 && val_pos + 0x42 <= pos + attr_len {
                        let namespace = rec[val_pos + 0x41];
                        if namespace == NS_DOS {
                            pos += attr_len;
                            continue;
                        }

                        let parent_ref =
                            u64::from_le_bytes(rec[val_pos..val_pos + 8].try_into().unwrap());
                        let parent_record = parent_ref & 0x0000_FFFF_FFFF_FFFF;

                        let name_chars = rec[val_pos + 0x40] as usize;
                        let name_end = val_pos + 0x42 + name_chars * 2;
                        if name_end <= pos + attr_len {
                            let name_u16: Vec<u16> = (0..name_chars)
                                .map(|j| {
                                    let o = val_pos + 0x42 + j * 2;
                                    u16::from_le_bytes([rec[o], rec[o + 1]])
                                })
                                .collect();
                            let name = String::from_utf16_lossy(&name_u16);

                            let dominated = best.as_ref().is_some_and(|b| {
                                ns_priority(b.namespace) >= ns_priority(namespace)
                            });
                            if !dominated {
                                best = Some(FileNameInfo {
                                    name,
                                    parent_record,
                                    namespace,
                                });
                            }
                        }
                    }
                }
            }
            ATTR_DATA if name_len == 0 => {
                // Unnamed $DATA — relevant for both base and extension records.
                if non_resident == 0 {
                    // Resident: data fits inside the MFT record.
                    if pos + 0x14 <= limit {
                        data_size =
                            u32::from_le_bytes(rec[pos + 0x10..pos + 0x14].try_into().unwrap())
                                as u64;
                        alloc_size = data_size;
                    }
                } else if pos + 0x38 <= limit {
                    data_size = u64::from_le_bytes(rec[pos + 0x30..pos + 0x38].try_into().unwrap());
                    // For compressed/sparse files, allocated_size (0x28) is the
                    // *virtual* allocation.  The actual on-disk size lives in
                    // compressed_size at offset 0x40 (present when compression_unit != 0).
                    let compression_unit = u16::from_le_bytes([rec[pos + 0x22], rec[pos + 0x23]]);
                    if compression_unit != 0 && pos + 0x48 <= limit {
                        alloc_size =
                            u64::from_le_bytes(rec[pos + 0x40..pos + 0x48].try_into().unwrap());
                    } else {
                        alloc_size =
                            u64::from_le_bytes(rec[pos + 0x28..pos + 0x30].try_into().unwrap());
                    }
                }
            }
            _ => {}
        }

        pos += attr_len;
    }

    if is_extension {
        // Only worth returning if we found an unnamed $DATA with sizes.
        if data_size > 0 || alloc_size > 0 {
            return Some(ParsedRecord::Extension {
                base_record,
                data_size,
                alloc_size,
            });
        }
        return None;
    }

    let info = best?;
    let (size_apparent, size_actual) = if is_directory {
        (0, 0)
    } else {
        (data_size, alloc_size)
    };

    Some(ParsedRecord::Base(MftEntry {
        parent_record_number: info.parent_record,
        name: info.name,
        is_directory,
        size_apparent,
        size_actual,
        mtime,
    }))
}

// ── Pass 1: stream MFT data and parse records ──────────────────────────

/// Read the MFT as a sequential stream and parse all records from memory.
///
/// This is dramatically faster than calling `ntfs.file(fs, i)` per record,
/// which must navigate the MFT data-run list on every call.
fn read_all_records<T: Read + Seek>(
    ntfs: &Ntfs,
    fs: &mut T,
    record_count: u64,
) -> Result<HashMap<u64, MftEntry>, HeftError> {
    let record_size = ntfs.file_record_size() as usize;

    // Open the $MFT's $DATA attribute for sequential reading.
    let mft_file = ntfs.file(fs, 0).map_err(ntfs_err)?;
    let data_item = mft_file
        .data(fs, "")
        .ok_or_else(|| HeftError::Other("MFT has no $DATA attribute".to_string()))?
        .map_err(ntfs_err)?;
    let data_attr = data_item.to_attribute().map_err(ntfs_err)?;
    let data_len = data_attr.value_length() as usize;
    let mut value = data_attr.value(fs).map_err(ntfs_err)?;

    let mut entries = HashMap::with_capacity(record_count as usize);
    let mut pending_extensions: Vec<(u64, u64, u64)> = Vec::new();

    // Read in 16 MB chunks (aligned to record_size).
    let chunk_records = (16 * 1024 * 1024) / record_size;
    let chunk_bytes = chunk_records * record_size;
    let mut buf = vec![0u8; chunk_bytes];

    let show_progress = std::io::stderr().is_terminal();
    let mut last_progress = Instant::now();
    let progress_interval = Duration::from_millis(100);

    let mut bytes_read_total: usize = 0;
    let mut records_parsed: u64 = 0;

    while bytes_read_total < data_len {
        let remaining = data_len - bytes_read_total;
        let to_read = remaining.min(chunk_bytes);

        // Fill the chunk from the MFT data stream.
        let mut filled = 0;
        while filled < to_read {
            let n = value
                .read(fs, &mut buf[filled..to_read])
                .map_err(|e| HeftError::Other(format!("MFT read error: {e}")))?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }

        // Parse each record in this chunk.
        let chunk_rec_count = filled / record_size;
        for j in 0..chunk_rec_count {
            let start = j * record_size;
            let record_num = records_parsed;
            records_parsed += 1;

            match parse_mft_record(&buf[start..start + record_size]) {
                Some(ParsedRecord::Base(entry)) => {
                    entries.insert(record_num, entry);
                }
                Some(ParsedRecord::Extension {
                    base_record,
                    data_size,
                    alloc_size,
                }) => {
                    // Try to merge immediately (base record usually has a lower number).
                    if let Some(base) = entries.get_mut(&base_record) {
                        if base.size_apparent == 0 && base.size_actual == 0 && !base.is_directory {
                            base.size_apparent = data_size;
                            base.size_actual = alloc_size;
                        }
                    } else {
                        // Base not seen yet — stash for later.
                        pending_extensions.push((base_record, data_size, alloc_size));
                    }
                }
                None => {}
            }
        }

        bytes_read_total += filled;

        if show_progress && last_progress.elapsed() >= progress_interval {
            let pct = (records_parsed as f64 / record_count as f64) * 100.0;
            eprint!("\rReading MFT: {records_parsed}/{record_count} ({pct:.0}%)...");
            let _ = std::io::stderr().flush();
            last_progress = Instant::now();
        }
    }

    // Merge any extension records whose base hadn't been seen yet.
    for (base_record, data_size, alloc_size) in pending_extensions {
        if let Some(base) = entries.get_mut(&base_record) {
            if base.size_apparent == 0 && base.size_actual == 0 && !base.is_directory {
                base.size_apparent = data_size;
                base.size_actual = alloc_size;
            }
        }
    }

    Ok(entries)
}

// ── Pass 2: build tree (unchanged) ─────────────────────────────────────

/// Build a TreeNode tree from flat MFT entries.
fn build_tree_from_entries(
    entries: &HashMap<u64, MftEntry>,
    root_record: u64,
    root_name: String,
) -> TreeNode {
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
    if !visited.insert(record) {
        return TreeNode {
            name: name.to_string(),
            size_apparent: 0,
            size_actual: 0,
            mtime: 0,
            count: 1,
            nlink: 1,
            dev: 0,
            ino: 0,
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
                            nlink: 1,
                            dev: 0,
                            ino: 0,
                            children: None,
                        });
                    }
                }
            }

            child_nodes.sort_by_key(|c| std::cmp::Reverse(c.size_actual));
            total_count += 1;

            TreeNode {
                name: name.to_string(),
                size_apparent: total_apparent,
                size_actual: total_actual,
                mtime: max_mtime,
                count: total_count,
                nlink: 1,
                dev: 0,
                ino: 0,
                children: Some(child_nodes),
            }
        }

        None => {
            if entry.map_or(true, |e| e.is_directory) {
                TreeNode {
                    name: name.to_string(),
                    size_apparent: 0,
                    size_actual: 0,
                    mtime: base_mtime,
                    count: 1,
                    nlink: 1,
                    dev: 0,
                    ino: 0,
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
                    nlink: 1,
                    dev: 0,
                    ino: 0,
                    children: None,
                }
            }
        }
    }
}

// ── Public API ──────────────────────────────────────────────────────────

/// Scan a volume's MFT and return a TreeNode plus the root label (e.g. "C:").
pub fn scan_volume(volume_spec: &str, _verbose: bool) -> Result<(TreeNode, String), HeftError> {
    let mut reader = volume::open_volume(volume_spec)?;
    let ntfs = Ntfs::new(&mut reader).map_err(ntfs_err)?;

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

    let start = Instant::now();
    let entries = read_all_records(&ntfs, &mut reader, record_count)?;
    eprint!("\r\x1b[2K");
    eprintln!(
        "Pass 1 complete: {} live entries in {:.1}s",
        entries.len(),
        start.elapsed().as_secs_f64()
    );

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
        let nt = 132_539_328_000_000_000u64;
        let unix = filetime_to_unix(nt);
        assert_eq!(unix, 1_609_459_200);

        let nt2 = 130_018_833_000_000_000u64;
        let unix2 = filetime_to_unix(nt2);
        assert_eq!(unix2, 1_357_409_700);

        assert_eq!(filetime_to_unix(0), 0);
        assert_eq!(filetime_to_unix(FILETIME_EPOCH_OFFSET), 0);

        let one_sec_after = FILETIME_EPOCH_OFFSET + INTERVALS_PER_SECOND;
        assert_eq!(filetime_to_unix(one_sec_after), 1);
    }

    #[test]
    fn test_apply_fixup_basic() {
        // Build a minimal 1024-byte "record" with 2 sectors.
        let mut rec = vec![0u8; 1024];
        rec[0..4].copy_from_slice(b"FILE");
        // USA at offset 0x30, count = 3 (USN + 2 sector entries).
        rec[0x04] = 0x30;
        rec[0x06] = 0x03;
        let usn: u16 = 0xABCD;
        rec[0x30..0x32].copy_from_slice(&usn.to_le_bytes()); // USN
        rec[0x32..0x34].copy_from_slice(&[0x11, 0x22]); // original bytes for sector 0
        rec[0x34..0x36].copy_from_slice(&[0x33, 0x44]); // original bytes for sector 1

        // Place USN at end of each sector.
        rec[510 - 2] = 0; // not at 510, at 510
        rec[510] = 0xCD;
        rec[511] = 0xAB;
        rec[1022] = 0xCD;
        rec[1023] = 0xAB;

        assert!(apply_fixup(&mut rec));
        assert_eq!(rec[510], 0x11);
        assert_eq!(rec[511], 0x22);
        assert_eq!(rec[1022], 0x33);
        assert_eq!(rec[1023], 0x44);
    }

    #[test]
    fn test_parse_record_not_in_use() {
        let mut rec = vec![0u8; 1024];
        rec[0..4].copy_from_slice(b"FILE");
        // Minimal valid USA.
        rec[0x04] = 0x30;
        rec[0x06] = 0x03;
        // Flags = 0 (not in use).
        rec[0x16] = 0x00;
        // Place USN at sector boundaries.
        // USN = 0, replacement entries = 0, stored values = 0 → fixup passes.

        assert!(parse_mft_record(&rec).is_none());
    }

    #[test]
    fn test_build_tree_from_entries() {
        let mut entries = HashMap::new();

        entries.insert(
            5,
            MftEntry {
                parent_record_number: 5,
                name: "C:".to_string(),
                is_directory: true,
                size_apparent: 0,
                size_actual: 0,
                mtime: 1000,
            },
        );

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

        assert_eq!(tree.name, "C:");
        assert_eq!(tree.size_apparent, 4096 + 100 + 2048);
        assert_eq!(tree.size_actual, 4096 + 512 + 4096);
        assert_eq!(tree.count, 5);
        assert_eq!(tree.mtime, 3000);

        let children = tree.children.as_ref().unwrap();
        assert_eq!(children.len(), 2);

        assert_eq!(children[0].name, "Users");
        assert_eq!(children[0].size_actual, 512 + 4096);
        assert_eq!(children[0].count, 3);

        assert_eq!(children[1].name, "pagefile.sys");
        assert_eq!(children[1].size_actual, 4096);
        assert_eq!(children[1].count, 1);

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
                parent_record_number: 999,
                name: "lost.dat".to_string(),
                is_directory: false,
                size_apparent: 1000,
                size_actual: 1000,
                mtime: 0,
            },
        );

        let tree = build_tree_from_entries(&entries, 5, "C:".to_string());
        assert_eq!(tree.count, 1);
        assert_eq!(tree.size_actual, 0);
    }
}
