use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "heft", about = "Fast disk usage scanner with mtime tracking")]
pub struct Cli {
    /// Path to the LMDB database (default: ~/.heft.db)
    #[arg(short = 'd', long = "database", global = true)]
    pub db_path: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Scan a directory and index it into the database
    Index {
        /// Root directory to scan
        path: PathBuf,

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

    /// Show database statistics
    Info,

    /// List contents of a directory from the database
    Ls {
        /// Directory path to list (default: indexed root)
        path: Option<PathBuf>,
    },

    /// Export directory tree as JSON
    Json {
        /// Root path to export (default: indexed root)
        path: Option<PathBuf>,

        /// Compression method
        #[arg(long, value_parser = ["gzip", "zstd"])]
        compress: Option<String>,

        /// Output file (default: stdout)
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
}
