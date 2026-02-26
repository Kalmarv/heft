use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::db::HeftDb;
use crate::error::{HeftError, Result};
use crate::tree::build_tree;

/// Export the tree as JSON with optional compression.
pub fn run_export(
    db: &HeftDb,
    root_path: &str,
    compress: Option<&str>,
    output: Option<&Path>,
) -> Result<()> {
    let tree = build_tree(db, root_path)?;
    let json = serde_json::to_string_pretty(&tree)?;
    let json_bytes = json.as_bytes();

    match (compress, output) {
        // No compression, stdout.
        (None, None) => {
            io::stdout().write_all(json_bytes)?;
            io::stdout().write_all(b"\n")?;
        }
        // No compression, file.
        (None, Some(path)) => {
            File::create(path)?.write_all(json_bytes)?;
        }
        // Gzip compression.
        (Some("gzip"), output_path) => {
            let writer: Box<dyn Write> = match output_path {
                Some(p) => Box::new(File::create(p)?),
                None => Box::new(io::stdout()),
            };
            let mut encoder = GzEncoder::new(writer, Compression::default());
            encoder.write_all(json_bytes)?;
            encoder.finish()?;
        }
        // Zstd compression.
        (Some("zstd"), output_path) => {
            let writer: Box<dyn Write> = match output_path {
                Some(p) => Box::new(File::create(p)?),
                None => Box::new(io::stdout()),
            };
            let mut encoder = zstd::Encoder::new(writer, 3)?;
            encoder.write_all(json_bytes)?;
            encoder.finish()?;
        }
        (Some(other), _) => {
            return Err(HeftError::Other(format!(
                "unsupported compression method: {other}"
            )));
        }
    }

    Ok(())
}
