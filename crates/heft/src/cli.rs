use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "heft", about = "Fast disk usage scanner with mtime tracking")]
pub struct Cli {
    /// LMDB map size in GiB (advanced, default: 4)
    #[arg(long, global = true, default_value_t = 4, hide = true)]
    pub map_size_gib: u64,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Scan a directory and produce a .heft file
    Scan {
        /// Root directory to scan
        path: PathBuf,

        /// Output file (default: <basename>.heft in cwd)
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Print every file path as it's scanned
        #[arg(short, long)]
        verbose: bool,

        /// Number of walker threads (default: available parallelism)
        #[arg(long)]
        threads: Option<usize>,

        /// Don't cross filesystem boundaries
        #[arg(long)]
        one_file_system: bool,

        /// Exclude paths containing this substring (can be repeated)
        #[arg(long)]
        exclude: Vec<String>,

        /// Read exclude patterns from a file, one per line
        #[arg(long)]
        exclude_from: Option<PathBuf>,
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
