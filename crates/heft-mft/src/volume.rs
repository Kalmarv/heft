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
pub fn open_volume(spec: &str) -> Result<BufReader<std::fs::File>, HeftError> {
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
    Ok(BufReader::new(file))
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
