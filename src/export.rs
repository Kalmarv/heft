use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;

use crate::db::HeftDb;
use crate::error::{HeftError, Result};
use crate::tree::build_tree;

/// Export the tree as JSON with optional compression.
///
/// Streams JSON directly into the output writer via `serde_json::to_writer_pretty`,
/// avoiding an intermediate `String` allocation that would duplicate the tree in memory.
pub fn run_export(
    db: &HeftDb,
    root_path: &str,
    compress: Option<&str>,
    output: Option<&Path>,
) -> Result<()> {
    let tree = build_tree(db, root_path)?;

    match (compress, output) {
        // No compression, stdout.
        (None, None) => {
            let mut out = io::stdout();
            serde_json::to_writer_pretty(&mut out, &tree)?;
            out.write_all(b"\n")?;
        }
        // No compression, file.
        (None, Some(path)) => {
            let out = File::create(path)?;
            serde_json::to_writer_pretty(out, &tree)?;
        }
        // Gzip compression.
        (Some("gzip"), output_path) => {
            let writer: Box<dyn Write> = match output_path {
                Some(p) => Box::new(File::create(p)?),
                None => Box::new(io::stdout()),
            };
            let mut encoder = GzEncoder::new(writer, Compression::default());
            serde_json::to_writer_pretty(&mut encoder, &tree)?;
            encoder.finish()?;
        }
        // Zstd compression.
        (Some("zstd"), output_path) => {
            let writer: Box<dyn Write> = match output_path {
                Some(p) => Box::new(File::create(p)?),
                None => Box::new(io::stdout()),
            };
            let mut encoder = zstd::Encoder::new(writer, 3)?;
            serde_json::to_writer_pretty(&mut encoder, &tree)?;
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
