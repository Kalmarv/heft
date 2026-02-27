#[cfg(not(unix))]
compile_error!("heft requires a Unix-like OS (Linux, macOS, etc.) — use heft-mft on Windows");

pub mod cli;
pub mod db;
pub mod entry;
pub mod error;
pub mod scanner;
pub mod tree;
