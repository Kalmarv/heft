use std::path::PathBuf;
use std::process;

use clap::{Parser, Subcommand};
use heft_format::display::{format_count, format_size};
use heft_format::pack;

mod mft;
mod volume;
#[cfg_attr(not(windows), allow(dead_code))]
mod windows;

#[derive(Parser)]
#[command(name = "heft-mft", about = "Windows MFT-based disk usage scanner")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a volume via MFT and produce a .heft file
    Scan {
        /// Drive letter or volume path (e.g. C:)
        volume: String,

        /// Output .heft file
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// List directory contents from a .heft file
    Ls {
        /// Path to the .heft file
        file: PathBuf,

        /// Directory path to list (default: root)
        path: Option<String>,
    },

    /// Show metadata from a .heft file
    Info {
        /// Path to the .heft file
        file: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    match cli.command {
        // No subcommand (double-clicked on Windows) → auto mode.
        None => {
            #[cfg(windows)]
            {
                let result = auto_mode();
                if let Err(e) = &result {
                    eprintln!("heft-mft: error: {e}");
                }
                windows::wait_for_keypress();
                if result.is_err() {
                    process::exit(1);
                }
            }
            #[cfg(not(windows))]
            {
                eprintln!("No command specified. Run with --help for usage.");
                process::exit(1);
            }
        }

        Some(cmd) => {
            if let Err(e) = run(cmd) {
                eprintln!("heft-mft: error: {e}");
                process::exit(1);
            }
        }
    }
}

/// Auto mode: detect OS drive, elevate if needed, scan, save dialog.
#[cfg(windows)]
fn auto_mode() -> Result<(), heft_format::HeftError> {
    if !windows::is_elevated() {
        eprintln!("Requesting Administrator privileges...");
        if windows::elevate_self() {
            // Elevated process launched — this one can exit.
            process::exit(0);
        } else {
            return Err(heft_format::HeftError::Other(
                "Administrator privileges are required to read the MFT".to_string(),
            ));
        }
    }

    let drive = volume::os_drive_letter();
    let volume_spec = format!("{drive}:");
    eprintln!("Scanning {volume_spec} ...");

    let (tree, root_path) = mft::scan_volume(&volume_spec, true)?;

    let default_name = format!("{drive}.heft");
    let output_path = windows::save_file_dialog(&default_name).ok_or_else(|| {
        heft_format::HeftError::Other("save cancelled".to_string())
    })?;

    pack::pack_tree(&tree, &root_path, &output_path)?;
    Ok(())
}

fn run(cmd: Command) -> Result<(), heft_format::HeftError> {
    match cmd {
        Command::Scan { volume, output } => {
            let (tree, root_path) = mft::scan_volume(&volume, false)?;
            let output_path = output.unwrap_or_else(|| {
                let base = root_path.trim_end_matches([':', '\\']);
                PathBuf::from(format!("{base}.heft"))
            });
            pack::pack_tree(&tree, &root_path, &output_path)?;
        }

        Command::Ls { file, path } => {
            let (meta, tree) = pack::load_tree(&file)?;
            let node = match &path {
                Some(p) => find_node(&tree, &meta.root, p)?,
                None => &tree,
            };
            print_ls(node)?;
        }

        Command::Info { file } => {
            let meta = pack::read_info(&file)?;
            println!("File:    {}", file.display());
            println!("Root:    {}", meta.root);
            println!("Version: {}", meta.heft_version);
            println!(
                "Data:    {} files  {} dirs  {} ({} bytes)",
                format_count(meta.file_count),
                format_count(meta.dir_count),
                format_size(meta.size_actual),
                meta.size_actual,
            );
            let file_size = std::fs::metadata(&file).map(|m| m.len()).unwrap_or(0);
            println!("Packed:  {}", format_size(file_size));
        }
    }

    Ok(())
}

/// Navigate a TreeNode tree by path, handling both `/` and `\` separators.
fn find_node<'a>(
    root: &'a heft_format::TreeNode,
    root_path: &str,
    target: &str,
) -> Result<&'a heft_format::TreeNode, heft_format::HeftError> {
    // Normalize backslashes to forward slashes for Windows path compat.
    let target_norm = target.replace('\\', "/");
    let root_trimmed = root_path.trim_end_matches('/');
    let target_trimmed = target_norm.trim_end_matches('/');

    if target_trimmed == root_trimmed || target_trimmed.is_empty() {
        return Ok(root);
    }

    let relative = if let Some(rest) = target_trimmed.strip_prefix(root_trimmed) {
        rest.trim_start_matches('/')
    } else {
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
        let children = current.children.as_ref().ok_or_else(|| {
            heft_format::HeftError::Other(format!("not a directory: {}", current.name))
        })?;
        current = children
            .iter()
            .find(|c| c.name == component)
            .ok_or_else(|| {
                heft_format::HeftError::Other(format!("path not found: {target}"))
            })?;
    }

    Ok(current)
}

fn print_ls(node: &heft_format::TreeNode) -> Result<(), heft_format::HeftError> {
    let children = node.children.as_ref().ok_or_else(|| {
        heft_format::HeftError::Other(format!("'{}' is a file, not a directory", node.name))
    })?;

    if children.is_empty() {
        println!("(empty directory)");
        return Ok(());
    }

    for child in children {
        let suffix = if child.children.is_some() { "/" } else { "" };
        println!(
            "{:>10}  {}{}",
            format_size(child.size_actual),
            child.name,
            suffix
        );
    }

    Ok(())
}
