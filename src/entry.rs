use crate::error::{DuxError, Result};

/// Type of filesystem entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryType {
    File = 0,
    Dir = 1,
    Symlink = 2,
    Other = 3,
}

impl EntryType {
    fn from_u8(v: u8) -> Result<Self> {
        match v {
            0 => Ok(EntryType::File),
            1 => Ok(EntryType::Dir),
            2 => Ok(EntryType::Symlink),
            3 => Ok(EntryType::Other),
            _ => Err(DuxError::Encoding(format!("invalid entry type: {v}"))),
        }
    }
}

/// A single filesystem entry stored in the database.
#[derive(Debug, Clone)]
pub struct EntryRecord {
    pub entry_type: EntryType,
    /// Modification time as seconds since Unix epoch.
    pub mtime: u64,
    /// Apparent (logical) size in bytes.
    pub size_apparent: u64,
    /// Actual (on-disk) size in bytes.
    pub size_actual: u64,
    /// Filename (just the basename, not the full path).
    pub name: String,
}

// ── LEB128 helpers ──────────────────────────────────────────────────────

fn write_varint(buf: &mut Vec<u8>, value: u64) {
    leb128::write::unsigned(buf, value).expect("vec write cannot fail");
}

fn read_varint(data: &[u8], pos: &mut usize) -> Result<u64> {
    let mut reader = &data[*pos..];
    let val = leb128::read::unsigned(&mut reader)
        .map_err(|e| DuxError::Encoding(format!("leb128 read: {e}")))?;
    // Advance pos by the number of bytes consumed.
    *pos = data.len() - reader.len();
    Ok(val)
}

// ── Batch encode / decode ───────────────────────────────────────────────

/// Encode a slice of entries into a compact binary format.
///
/// Layout per entry:
///   entry_type  (1 byte)
///   mtime       (LEB128)
///   size_apparent (LEB128)
///   size_actual   (LEB128)
///   name_len      (LEB128)
///   name_bytes    (UTF-8, name_len bytes)
pub fn encode_many(entries: &[EntryRecord]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(entries.len() * 32);
    for e in entries {
        buf.push(e.entry_type as u8);
        write_varint(&mut buf, e.mtime);
        write_varint(&mut buf, e.size_apparent);
        write_varint(&mut buf, e.size_actual);
        let name_bytes = e.name.as_bytes();
        write_varint(&mut buf, name_bytes.len() as u64);
        buf.extend_from_slice(name_bytes);
    }
    buf
}

/// Decode a byte slice back into a vector of entries.
pub fn decode_many(data: &[u8]) -> Result<Vec<EntryRecord>> {
    let mut entries = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let entry_type = EntryType::from_u8(data[pos])?;
        pos += 1;
        let mtime = read_varint(data, &mut pos)?;
        let size_apparent = read_varint(data, &mut pos)?;
        let size_actual = read_varint(data, &mut pos)?;
        let name_len = read_varint(data, &mut pos)? as usize;
        if pos + name_len > data.len() {
            return Err(DuxError::Encoding("truncated name".into()));
        }
        // Fast path: most filenames are valid UTF-8, avoid the lossy Cow allocation.
        let name_bytes = &data[pos..pos + name_len];
        let name = match std::str::from_utf8(name_bytes) {
            Ok(s) => s.to_owned(),
            Err(_) => String::from_utf8_lossy(name_bytes).into_owned(),
        };
        pos += name_len;
        entries.push(EntryRecord {
            entry_type,
            mtime,
            size_apparent,
            size_actual,
            name,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty() {
        let encoded = encode_many(&[]);
        let decoded = decode_many(&encoded).unwrap();
        assert!(decoded.is_empty());
    }

    #[test]
    fn round_trip_single() {
        let entries = vec![EntryRecord {
            entry_type: EntryType::File,
            mtime: 1700000000,
            size_apparent: 4096,
            size_actual: 8192,
            name: "hello.txt".into(),
        }];
        let encoded = encode_many(&entries);
        let decoded = decode_many(&encoded).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].entry_type, EntryType::File);
        assert_eq!(decoded[0].mtime, 1700000000);
        assert_eq!(decoded[0].size_apparent, 4096);
        assert_eq!(decoded[0].size_actual, 8192);
        assert_eq!(decoded[0].name, "hello.txt");
    }

    #[test]
    fn round_trip_multiple() {
        let entries = vec![
            EntryRecord {
                entry_type: EntryType::Dir,
                mtime: 0,
                size_apparent: 0,
                size_actual: 4096,
                name: "subdir".into(),
            },
            EntryRecord {
                entry_type: EntryType::File,
                mtime: u64::MAX,
                size_apparent: u64::MAX,
                size_actual: u64::MAX,
                name: "big_values.bin".into(),
            },
            EntryRecord {
                entry_type: EntryType::Symlink,
                mtime: 100,
                size_apparent: 50,
                size_actual: 0,
                name: "link → target".into(),
            },
        ];
        let encoded = encode_many(&entries);
        let decoded = decode_many(&encoded).unwrap();
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].entry_type, EntryType::Dir);
        assert_eq!(decoded[0].name, "subdir");
        assert_eq!(decoded[1].size_apparent, u64::MAX);
        assert_eq!(decoded[2].entry_type, EntryType::Symlink);
        assert_eq!(decoded[2].name, "link → target");
    }

    #[test]
    fn unicode_name() {
        let entries = vec![EntryRecord {
            entry_type: EntryType::File,
            mtime: 42,
            size_apparent: 100,
            size_actual: 512,
            name: "日本語ファイル.txt".into(),
        }];
        let encoded = encode_many(&entries);
        let decoded = decode_many(&encoded).unwrap();
        assert_eq!(decoded[0].name, "日本語ファイル.txt");
    }
}
