use std::process;

use clap::{Parser, Subcommand};
use heft_format::display::{format_count, format_size};
use heft_format::pack;

#[derive(Parser)]
#[command(name = "heft-mft", about = "Windows MFT-based disk usage scanner")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Scan a volume via MFT and produce a .heft file
    Scan {
        /// Drive letter or volume path (e.g. C:)
        volume: String,

        /// Output .heft file
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
    },

    /// List directory contents from a .heft file
    Ls {
        /// Path to the .heft file
        file: std::path::PathBuf,

        /// Directory path to list (default: root)
        path: Option<String>,
    },

    /// Show metadata from a .heft file
    Info {
        /// Path to the .heft file
        file: std::path::PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("heft-mft: error: {e}");
        process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), heft_format::HeftError> {
    match cli.command {
        Command::Scan { volume, .. } => {
            eprintln!(
                "MFT scanning for volume '{}' is not yet implemented.\n\
                 This feature requires Windows and the ntfs crate.",
                volume
            );
            process::exit(2);
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

fn find_node<'a>(
    root: &'a heft_format::TreeNode,
    root_path: &str,
    target: &str,
) -> Result<&'a heft_format::TreeNode, heft_format::HeftError> {
    let root_trimmed = root_path.trim_end_matches('/');
    let target_trimmed = target.trim_end_matches('/');

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
