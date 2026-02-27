use std::io::BufReader;

use heft_format::HeftError;

/// Normalize a volume spec ("C", "C:", "c:\") into a device path like `\\.\C:`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn volume_device_path(spec: &str) -> String {
    let letter = spec
        .trim_end_matches([':', '\\', '/'])
        .chars()
        .next()
        .unwrap_or('C')
        .to_ascii_uppercase();
    format!(r"\\.\{letter}:")
}

#[cfg(windows)]
pub fn open_volume(spec: &str) -> Result<BufReader<SectorReader>, HeftError> {
    let path = volume_device_path(spec);
    let file = std::fs::File::open(&path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            HeftError::Other(format!(
                "access denied opening {path} — run as Administrator"
            ))
        } else {
            HeftError::Io(e)
        }
    })?;
    Ok(BufReader::new(SectorReader::new(file, 4096)))
}

#[cfg(not(windows))]
pub fn open_volume(_spec: &str) -> Result<BufReader<std::fs::File>, HeftError> {
    Err(HeftError::Other(
        "MFT scanning requires Windows (NTFS volumes are not accessible on this platform)"
            .to_string(),
    ))
}

/// Detect the Windows OS drive letter from %SystemDrive% (e.g. 'C').
/// Returns 'C' as fallback if the env var is missing (non-Windows or unusual setup).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn os_drive_letter() -> char {
    std::env::var("SystemDrive")
        .ok()
        .and_then(|s| s.chars().next())
        .map(|c| c.to_ascii_uppercase())
        .unwrap_or('C')
}

/// Adapter for raw volume handles on Windows.
///
/// Raw device I/O (`\\.\X:`) requires all reads to be sector-aligned in both
/// offset and length.  This wrapper reads full sectors and serves the requested
/// byte ranges from an internal buffer, presenting a normal `Read + Seek`
/// interface that the `ntfs` crate (and `BufReader`) can use directly.
#[cfg(windows)]
pub struct SectorReader {
    inner: std::fs::File,
    sector_size: usize,
    /// Current logical position (what the consumer sees).
    pos: u64,
    /// Sector-aligned buffer.
    buf: Vec<u8>,
    /// Range of the file currently held in `buf`: [buf_start .. buf_start + buf_len).
    buf_start: u64,
    buf_len: usize,
}

#[cfg(windows)]
impl SectorReader {
    pub fn new(inner: std::fs::File, sector_size: usize) -> Self {
        assert!(sector_size > 0 && sector_size.is_power_of_two());
        Self {
            inner,
            sector_size,
            pos: 0,
            buf: Vec::new(),
            buf_start: 0,
            buf_len: 0,
        }
    }

    /// Ensure `buf` contains data covering `self.pos`.  Returns the offset
    /// into `buf` where `self.pos` starts, or `None` at EOF.
    fn fill(&mut self) -> std::io::Result<Option<usize>> {
        let ss = self.sector_size as u64;
        let aligned_start = self.pos / ss * ss;
        let offset_in_buf = (self.pos - aligned_start) as usize;

        // Already buffered?
        if aligned_start == self.buf_start
            && self.buf_len > 0
            && offset_in_buf < self.buf_len
        {
            return Ok(Some(offset_in_buf));
        }

        // Read a chunk of sectors.  Use a generous read-ahead (2048 sectors = 8 MB)
        // to minimize syscalls during sequential MFT reads.
        let read_len = self.sector_size * 2048;
        self.buf.resize(read_len, 0);

        use std::io::{Read, Seek, SeekFrom};
        self.inner.seek(SeekFrom::Start(aligned_start))?;
        let mut total = 0;
        while total < read_len {
            match self.inner.read(&mut self.buf[total..read_len]) {
                Ok(0) => break,
                Ok(n) => total += n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }

        self.buf_start = aligned_start;
        self.buf_len = total;

        if offset_in_buf >= total {
            Ok(None) // EOF
        } else {
            Ok(Some(offset_in_buf))
        }
    }
}

#[cfg(windows)]
impl std::io::Read for SectorReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }

        let offset = match self.fill()? {
            Some(o) => o,
            None => return Ok(0),
        };

        let available = self.buf_len - offset;
        let n = out.len().min(available);
        out[..n].copy_from_slice(&self.buf[offset..offset + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

#[cfg(windows)]
impl std::io::Seek for SectorReader {
    fn seek(&mut self, from: std::io::SeekFrom) -> std::io::Result<u64> {
        let new_pos = match from {
            std::io::SeekFrom::Start(p) => p,
            std::io::SeekFrom::Current(delta) => {
                if delta >= 0 {
                    self.pos + delta as u64
                } else {
                    self.pos.checked_sub((-delta) as u64).ok_or_else(|| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidInput,
                            "seek before start of file",
                        )
                    })?
                }
            }
            std::io::SeekFrom::End(_) => {
                // Raw volumes don't reliably support SEEK_END; the ntfs crate
                // doesn't use it, so we can leave it unsupported.
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "SeekFrom::End not supported on raw volumes",
                ));
            }
        };
        self.pos = new_pos;
        Ok(new_pos)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_volume_device_path() {
        assert_eq!(volume_device_path("C"), r"\\.\C:");
        assert_eq!(volume_device_path("C:"), r"\\.\C:");
        assert_eq!(volume_device_path(r"c:\"), r"\\.\C:");
        assert_eq!(volume_device_path("D"), r"\\.\D:");
        assert_eq!(volume_device_path("d:"), r"\\.\D:");
    }
}
