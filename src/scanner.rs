use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::Instant;

use jwalk::{Parallelism, WalkDir};

use crate::db::DuxDb;
use crate::entry::{encode_many, EntryRecord, EntryType};
use crate::error::Result;

/// Maximum number of parent directories to accumulate before flushing to DB.
const BATCH_FLUSH_THRESHOLD: usize = 10_000;

pub struct ScanOptions {
    pub verbose: bool,
    pub threads: Option<usize>,
    pub one_file_system: bool,
    pub exclude: Vec<String>,
}

struct ScanState {
    dir_entries: HashMap<String, Vec<EntryRecord>>,
    file_count: u64,
    dir_count: u64,
    total_size: u64,
}

/// Run the index operation: walk a directory tree and store results in the database.
pub fn run_index(root: &Path, db: &DuxDb, opts: &ScanOptions) -> Result<()> {
    let root = fs::canonicalize(root).map_err(|e| {
        crate::error::DuxError::Other(format!("cannot access '{}': {e}", root.display()))
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

    let mut state = ScanState {
        dir_entries: HashMap::new(),
        file_count: 0,
        dir_count: 0,
        total_size: 0,
    };

    // Build walker — two branches because process_read_dir changes the generic type.
    if opts.one_file_system {
        let root_dev = fs::metadata(&root).map(|m| m.dev()).unwrap_or(0);
        let walker = WalkDir::new(&root)
            .skip_hidden(false)
            .follow_links(false)
            .parallelism(parallelism)
            .process_read_dir(move |_depth, _path, _state, children| {
                children.retain(|entry| {
                    if let Ok(ref e) = entry {
                        if let Ok(meta) = e.path().symlink_metadata() {
                            return meta.dev() == root_dev;
                        }
                    }
                    true
                });
            });
        for result in walker {
            match result {
                Ok(entry) => process_path(&root, &entry.path(), opts, db, &mut state)?,
                Err(e) => eprintln!("dux: warning: {e}"),
            }
        }
    } else {
        let walker = WalkDir::new(&root)
            .skip_hidden(false)
            .follow_links(false)
            .parallelism(parallelism);
        for result in walker {
            match result {
                Ok(entry) => process_path(&root, &entry.path(), opts, db, &mut state)?,
                Err(e) => eprintln!("dux: warning: {e}"),
            }
        }
    }

    // Flush remaining entries.
    if !state.dir_entries.is_empty() {
        flush_batch(db, &mut state.dir_entries)?;
    }

    // Store the root sentinel.
    db.put_root(&root_str)?;

    let elapsed = start.elapsed();
    eprintln!(
        "Indexed {} files in {} directories ({}) in {:.1}s",
        format_count(state.file_count),
        format_count(state.dir_count),
        format_size(state.total_size),
        elapsed.as_secs_f64()
    );

    Ok(())
}

fn process_path(
    root: &Path,
    path: &Path,
    opts: &ScanOptions,
    db: &DuxDb,
    state: &mut ScanState,
) -> Result<()> {
    // Skip excluded paths.
    if should_exclude(path, &opts.exclude) {
        return Ok(());
    }

    let metadata = match path.symlink_metadata() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("dux: warning: {}: {e}", path.display());
            return Ok(());
        }
    };

    let entry_type = if metadata.is_file() {
        EntryType::File
    } else if metadata.is_dir() {
        EntryType::Dir
    } else if metadata.file_type().is_symlink() {
        EntryType::Symlink
    } else {
        EntryType::Other
    };

    let mtime = metadata.mtime().max(0) as u64;
    let size_apparent = metadata.len();
    let size_actual = metadata.blocks() * 512;

    // The root directory itself: just track as a dir, don't add to a parent.
    if path == root {
        state.dir_count += 1;
        return Ok(());
    }

    let name = path
        .file_name()
        .map(|n: &std::ffi::OsStr| n.to_string_lossy().to_string())
        .unwrap_or_default();

    let parent = path
        .parent()
        .map(|p: &Path| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let record = EntryRecord {
        entry_type,
        mtime,
        size_apparent,
        size_actual,
        name,
    };

    if entry_type == EntryType::Dir {
        state.dir_count += 1;
    } else {
        state.file_count += 1;
        state.total_size += size_actual;
    }

    if opts.verbose && entry_type != EntryType::Dir {
        eprintln!("{} ({})", path.display(), format_size(size_apparent));
    }

    state.dir_entries.entry(parent).or_default().push(record);

    // Flush accumulated entries periodically.
    if state.dir_entries.len() >= BATCH_FLUSH_THRESHOLD {
        flush_batch(db, &mut state.dir_entries)?;
    }

    Ok(())
}

fn flush_batch(db: &DuxDb, dir_entries: &mut HashMap<String, Vec<EntryRecord>>) -> Result<()> {
    let batch: Vec<(String, Vec<u8>)> = dir_entries
        .drain()
        .map(|(path, entries)| (path, encode_many(&entries)))
        .collect();
    db.put_batch(batch.iter().map(|(p, e)| (p.as_str(), e.clone())))?;
    Ok(())
}

fn should_exclude(path: &Path, patterns: &[String]) -> bool {
    if patterns.is_empty() {
        return false;
    }
    let path_str = path.to_string_lossy();
    for pattern in patterns {
        if path_str.contains(pattern.as_str()) {
            return true;
        }
    }
    false
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

fn format_count(n: u64) -> String {
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
