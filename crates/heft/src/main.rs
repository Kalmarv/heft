use std::fs;
use std::path::PathBuf;
use std::process;

use clap::Parser;
use heft_format::display::{format_count, format_size};
use heft_format::pack;

use heft::cli::{Cli, Command};
use heft::db::HeftDb;
use heft::error;
use heft::scanner::{self, ScanOptions};
use heft::tree;

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("heft: error: {e}");
        process::exit(1);
    }
}

fn run(cli: Cli) -> error::Result<()> {
    let map_size_gib = cli.map_size_gib;

    match cli.command {
        Command::Scan {
            path,
            output,
            verbose,
            threads,
            one_file_system,
            exclude,
            exclude_from,
        } => {
            let mut exclude_patterns = exclude;
            if let Some(file) = exclude_from {
                let contents = fs::read_to_string(&file).map_err(|e| {
                    error::HeftError::Other(format!(
                        "cannot read exclude file '{}': {e}",
                        file.display()
                    ))
                })?;
                for line in contents.lines() {
                    let line = line.trim();
                    if !line.is_empty() && !line.starts_with('#') {
                        exclude_patterns.push(line.to_string());
                    }
                }
            }

            // Resolve the scan path to an absolute path.
            let abs_path = std::path::absolute(&path).map_err(|e| {
                error::HeftError::Other(format!("cannot resolve '{}': {e}", path.display()))
            })?;
            let root_str = abs_path.to_string_lossy().to_string();
            let root_str = if root_str.len() > 1 {
                root_str.trim_end_matches('/').to_string()
            } else {
                root_str
            };

            // Derive output path: -o flag or <basename>.heft in cwd.
            let output_path = output.unwrap_or_else(|| {
                let basename = std::path::Path::new(&root_str)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "root".to_string());
                PathBuf::from(format!("{basename}.heft"))
            });

            // Create a transient LMDB in a tempdir.
            let tmpdir = tempfile::TempDir::new().map_err(|e| {
                error::HeftError::Other(format!("cannot create temp directory: {e}"))
            })?;

            let db = HeftDb::open(tmpdir.path(), map_size_gib)?;
            let opts = ScanOptions {
                verbose,
                threads,
                one_file_system,
                exclude: exclude_patterns,
            };
            scanner::run_index(&abs_path, &db, &opts)?;

            // Build tree from DB, then close DB before packing.
            let t = tree::build_tree(&db, &root_str)?;
            drop(db);

            pack::pack_tree(&t, &root_str, &output_path)?;
            // tmpdir auto-cleans on drop
        }

        Command::Ls { file, path } => {
            let (meta, t) = pack::load_tree(&file)?;
            let node = match &path {
                Some(p) => find_node(&t, &meta.root, p)?,
                None => &t,
            };
            run_ls(node)?;
        }

        Command::Info { file } => {
            let meta = pack::read_info(&file)?;
            println!("File:    {}", file.display());
            println!("Root:    {}", meta.root);
            println!("Version: {}", meta.heft_version);
            println!(
                "Data:    {} files  {} dirs  {}",
                format_count(meta.file_count),
                format_count(meta.dir_count),
                format_size(meta.size_actual),
            );
            let file_size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            println!("Packed:  {}", format_size(file_size));
        }
    }

    Ok(())
}

// ── Tree navigation ─────────────────────────────────────────────────────

/// Navigate the tree to find the node at `target_path`.
///
/// Strips the root prefix from the target, then walks tree children
/// matching each remaining path component.
fn find_node<'a>(
    root: &'a heft_format::TreeNode,
    root_path: &str,
    target: &str,
) -> error::Result<&'a heft_format::TreeNode> {
    let root_trimmed = root_path.trim_end_matches('/');
    let target_trimmed = target.trim_end_matches('/');

    // If the target is the root itself, return the root node.
    if target_trimmed == root_trimmed || target_trimmed.is_empty() {
        return Ok(root);
    }

    // Strip the root prefix to get the relative path.
    let relative = if let Some(rest) = target_trimmed.strip_prefix(root_trimmed) {
        rest.trim_start_matches('/')
    } else {
        // Target doesn't start with root — treat it as a relative path.
        target_trimmed.trim_start_matches('/')
    };

    if relative.is_empty() {
        return Ok(root);
    }

    let mut current = root;
    for component in relative.split('/') {
        if component.is_empty() {
            continue;
        }
        let children = current
            .children
            .as_ref()
            .ok_or_else(|| error::HeftError::Other(format!("not a directory: {}", current.name)))?;
        current = children
            .iter()
            .find(|c| c.name == component)
            .ok_or_else(|| error::HeftError::Other(format!("path not found: {target}")))?;
    }

    Ok(current)
}

// ── Listing ─────────────────────────────────────────────────────────────

/// Display a directory listing from a TreeNode.
fn run_ls(node: &heft_format::TreeNode) -> error::Result<()> {
    let children = node.children.as_ref().ok_or_else(|| {
        error::HeftError::Other(format!("'{}' is a file, not a directory", node.name))
    })?;

    if children.is_empty() {
        println!("(empty directory)");
        return Ok(());
    }

    struct Row {
        name: String,
        apparent: u64,
        actual: u64,
        mtime: u64,
    }

    let mut rows: Vec<Row> = children
        .iter()
        .map(|child| {
            let suffix = if child.children.is_some() { "/" } else { "" };
            Row {
                name: format!("{}{}", child.name, suffix),
                apparent: child.size_apparent,
                actual: child.size_actual,
                mtime: child.mtime,
            }
        })
        .collect();

    // Sort by actual size descending.
    rows.sort_unstable_by_key(|r| std::cmp::Reverse(r.actual));

    // Format and compute column widths.
    let formatted: Vec<(String, String, String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.name.clone(),
                format_size(r.apparent),
                format_size(r.actual),
                format_mtime(r.mtime),
            )
        })
        .collect();

    let name_w = formatted.iter().map(|r| r.0.len()).max().unwrap_or(0);
    let app_w = formatted.iter().map(|r| r.1.len()).max().unwrap_or(0);
    let act_w = formatted.iter().map(|r| r.2.len()).max().unwrap_or(0);

    for r in &formatted {
        println!(
            "{:<name_w$}  {:>app_w$}  {:>act_w$}  {}",
            r.0, r.1, r.2, r.3
        );
    }

    Ok(())
}

fn format_mtime(epoch_secs: u64) -> String {
    let days = epoch_secs / 86400;
    let time_of_day = epoch_secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;

    let days_i64 = days.min(i64::MAX as u64) as i64;
    let (year, month, day) = days_to_ymd(days_i64);
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}Z")
}

fn days_to_ymd(mut days: i64) -> (i64, i64, i64) {
    // Algorithm from Howard Hinnant's date library.
    days += 719468;
    let era = if days >= 0 { days } else { days - 146096 } / 146097;
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}
