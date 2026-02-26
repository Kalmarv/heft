use std::fs;
use std::path::PathBuf;
use std::process;

use clap::Parser;

use dux::cli::{Cli, Command};
use dux::db::DuxDb;
use dux::entry;
use dux::error;
use dux::export;
use dux::scanner::{self, format_count, format_size, ScanOptions};
use dux::tree;

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("dux: error: {e}");
        process::exit(1);
    }
}

fn run(cli: Cli) -> error::Result<()> {
    let db_path = cli.db_path.unwrap_or_else(DuxDb::default_path);

    match cli.command {
        Command::Index {
            path,
            verbose,
            threads,
            one_file_system,
            exclude,
            exclude_from,
        } => {
            let mut exclude_patterns = exclude;
            if let Some(file) = exclude_from {
                let contents = fs::read_to_string(&file).map_err(|e| {
                    error::DuxError::Other(format!(
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

            let db = DuxDb::open(&db_path)?;
            let opts = ScanOptions {
                verbose,
                threads,
                one_file_system,
                exclude: exclude_patterns,
            };
            scanner::run_index(&path, &db, &opts)?;
        }

        Command::Info => {
            let db = DuxDb::open(&db_path)?;
            let root = db.get_root()?;
            let disk = db.disk_size()?;

            if let Some(ref root_path) = root {
                let rtxn = db.read_txn()?;
                let agg = tree::aggregate_sizes(&db, &rtxn, root_path)?;
                println!(
                    "{}  {} files  {} dirs  {}",
                    root_path,
                    format_count(agg.file_count),
                    format_count(agg.dir_count),
                    format_size(agg.size_actual),
                );
            } else {
                println!("(no index — run `dux index <path>` first)");
            }
            println!("Database: {} ({})", db_path.display(), format_size(disk));
        }

        Command::Ls { path } => {
            let db = DuxDb::open(&db_path)?;
            let dir_path = resolve_path(&db, path)?;
            run_ls(&db, &dir_path)?;
        }

        Command::Json {
            path,
            compress,
            output,
        } => {
            let db = DuxDb::open(&db_path)?;
            let root_path = resolve_path(&db, path)?;
            export::run_export(
                &db,
                &root_path,
                compress.as_deref(),
                output.as_deref(),
            )?;
        }
    }

    Ok(())
}

/// Resolve a user-provided path, or fall back to the indexed root.
fn resolve_path(db: &DuxDb, path: Option<PathBuf>) -> error::Result<String> {
    match path {
        Some(p) => {
            let canonical = fs::canonicalize(&p).map_err(|e| {
                error::DuxError::Other(format!("cannot resolve '{}': {e}", p.display()))
            })?;
            Ok(canonical.to_string_lossy().to_string())
        }
        None => db.get_root()?.ok_or_else(|| {
            error::DuxError::Other("no index found — run `dux index <path>` first".into())
        }),
    }
}

/// Display a directory listing with human-readable sizes.
fn run_ls(db: &DuxDb, dir_path: &str) -> error::Result<()> {
    // Single read transaction for the entire ls operation.
    let rtxn = db.read_txn()?;

    let entries = db.get_dir_in_txn(&rtxn, dir_path)?.ok_or_else(|| {
        error::DuxError::Other(format!("directory not found in database: {dir_path}"))
    })?;

    if entries.is_empty() {
        println!("(empty directory)");
        return Ok(());
    }

    struct Row {
        name: String,
        apparent: u64,
        actual: u64,
        mtime: u64,
    }

    let mut rows: Vec<Row> = Vec::with_capacity(entries.len());
    for e in &entries {
        let suffix = match e.entry_type {
            entry::EntryType::Dir => "/",
            entry::EntryType::Symlink => "@",
            _ => "",
        };

        // Use lightweight aggregate for dirs — no TreeNode allocation.
        let (apparent, actual) = if e.entry_type == entry::EntryType::Dir {
            let child_path = format!("{}/{}", dir_path, e.name);
            match tree::aggregate_sizes(db, &rtxn, &child_path) {
                Ok(agg) => (agg.size_apparent, agg.size_actual),
                Err(_) => (e.size_apparent, e.size_actual),
            }
        } else {
            (e.size_apparent, e.size_actual)
        };

        rows.push(Row {
            name: format!("{}{}", e.name, suffix),
            apparent,
            actual,
            mtime: e.mtime,
        });
    }

    // Sort by actual size descending.
    rows.sort_unstable_by(|a, b| b.actual.cmp(&a.actual));

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
    // Simple ISO-ish date without pulling in chrono.
    let secs = epoch_secs as i64;
    // Use a basic approach: days since epoch.
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;

    // Compute year/month/day from days since epoch (1970-01-01).
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}")
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
