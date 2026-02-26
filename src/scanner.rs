use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use crossbeam_channel::bounded;
use jwalk::{Parallelism, WalkDir};

use crate::db::HeftDb;
use crate::entry::{encode_many, EntryRecord, EntryType};
use crate::error::Result;

/// Commit LMDB write transaction every N directories.
const TXN_BATCH_SIZE: usize = 1_000;

pub struct ScanOptions {
    pub verbose: bool,
    pub threads: Option<usize>,
    pub one_file_system: bool,
    pub exclude: Vec<String>,
}

/// Channel message: one fully-encoded directory.
struct DirBatch {
    parent_path: String,
    encoded: Vec<u8>,
    file_count: u64,
    dir_count: u64,
    size_actual: u64,
}

/// Run the index operation: walk a directory tree and store results in the database.
pub fn run_index(root: &Path, db: &HeftDb, opts: &ScanOptions) -> Result<()> {
    let root = fs::canonicalize(root).map_err(|e| {
        crate::error::HeftError::Other(format!("cannot access '{}': {e}", root.display()))
    })?;
    let root_str = root.to_string_lossy().to_string();

    db.clear()?;

    let start = Instant::now();

    let parallelism = match opts.threads {
        Some(n) => Parallelism::RayonNewPool(n),
        None => Parallelism::RayonDefaultPool {
            busy_timeout: std::time::Duration::from_secs(1),
        },
    };

    // Shared state for the closure.
    let exclude_patterns: Arc<Vec<String>> = Arc::new(opts.exclude.clone());
    let root_dev = if opts.one_file_system {
        fs::metadata(&root).map(|m| m.dev()).unwrap_or(0)
    } else {
        0
    };
    let one_file_system = opts.one_file_system;
    let verbose = opts.verbose;

    // Bounded channel: worker threads → main thread.
    let (tx, rx) = bounded::<DirBatch>(256);

    let walker = WalkDir::new(&root)
        .skip_hidden(false)
        .follow_links(false)
        .parallelism(parallelism)
        .process_read_dir(move |depth, path, _state, children| {
            // depth == None means the virtual parent of root — skip.
            let depth = match depth {
                Some(d) => d,
                None => return,
            };
            // At depth 0, path is the root itself. We stat its children.
            let _ = depth;

            let parent_str = path.to_string_lossy().to_string();
            let exclude = &*exclude_patterns;

            let mut entries = Vec::with_capacity(children.len());
            let mut batch_file_count: u64 = 0;
            let mut batch_dir_count: u64 = 0;
            let mut batch_size_actual: u64 = 0;

            for child in children.iter_mut() {
                let dir_entry = match child {
                    Ok(ref mut e) => e,
                    Err(_) => continue,
                };

                let child_path = dir_entry.path();

                // Exclusion check.
                if !exclude.is_empty() {
                    let child_str = child_path.to_string_lossy();
                    let excluded = exclude.iter().any(|pat| child_str.contains(pat.as_str()));
                    if excluded {
                        // Prevent descent into excluded directories.
                        dir_entry.read_children_path = None;
                        continue;
                    }
                }

                // Single stat call per entry.
                let metadata = match child_path.symlink_metadata() {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!("heft: warning: {}: {e}", child_path.display());
                        continue;
                    }
                };

                // Cross-device check (one_file_system).
                if one_file_system && metadata.dev() != root_dev {
                    dir_entry.read_children_path = None;
                    continue;
                }

                let ft = metadata.file_type();
                let entry_type = if ft.is_file() {
                    EntryType::File
                } else if ft.is_dir() {
                    EntryType::Dir
                } else if ft.is_symlink() {
                    EntryType::Symlink
                } else {
                    EntryType::Other
                };

                let mtime = metadata.mtime().max(0) as u64;
                let size_apparent = metadata.len();
                let size_actual = metadata.blocks() * 512;

                let name = child_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_owned();

                if verbose && entry_type != EntryType::Dir {
                    eprintln!("{} ({})", child_path.display(), format_size(size_apparent));
                }

                if entry_type == EntryType::Dir {
                    batch_dir_count += 1;
                } else {
                    batch_file_count += 1;
                    batch_size_actual += size_actual;
                }

                entries.push(EntryRecord {
                    entry_type,
                    mtime,
                    size_apparent,
                    size_actual,
                    name,
                });
            }

            if !entries.is_empty() {
                let encoded = encode_many(&entries);
                if tx.send(DirBatch {
                    parent_path: parent_str,
                    encoded,
                    file_count: batch_file_count,
                    dir_count: batch_dir_count,
                    size_actual: batch_size_actual,
                }).is_err() {
                    eprintln!("heft: warning: receiver dropped, scan results lost");
                    return;
                }
            }
        });

    // Spawn thread to drive walker iteration (entries are discarded — the
    // callback does the real work).
    let walker_handle = std::thread::spawn(move || {
        for result in walker {
            if let Err(e) = result {
                eprintln!("heft: warning: {e}");
            }
        }
    });

    // Main thread: receive encoded directories, batch-write to LMDB.
    let mut file_count: u64 = 0;
    let mut dir_count: u64 = 0;
    let mut total_size: u64 = 0;
    let mut dirs_in_txn: usize = 0;
    let mut wtxn = db.write_txn()?;

    for batch in &rx {
        db.put_dir_in_txn(&mut wtxn, &batch.parent_path, &batch.encoded)?;
        file_count += batch.file_count;
        dir_count += batch.dir_count;
        total_size += batch.size_actual;
        dirs_in_txn += 1;

        if dirs_in_txn >= TXN_BATCH_SIZE {
            wtxn.commit()?;
            wtxn = db.write_txn()?;
            dirs_in_txn = 0;
        }
    }

    // Commit remaining writes.
    wtxn.commit()?;

    // Wait for walker thread.
    walker_handle.join().expect("walker thread panicked");

    // +1 for the root directory itself.
    dir_count += 1;

    // Store the root sentinel.
    db.put_root(&root_str)?;

    let elapsed = start.elapsed();
    eprintln!(
        "Indexed {} files in {} directories ({}) in {:.1}s",
        format_count(file_count),
        format_count(dir_count),
        format_size(total_size),
        elapsed.as_secs_f64()
    );

    Ok(())
}

// ── Formatting helpers ──────────────────────────────────────────────────

pub fn format_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;

    if bytes >= TIB {
        format!("{:.1} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

pub fn format_count(n: u64) -> String {
    if n < 1000 {
        return n.to_string();
    }
    let s = n.to_string();
    let mut result = String::new();
    for (i, ch) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(ch);
    }
    result.chars().rev().collect()
}
