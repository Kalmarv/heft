use std::fs;
use std::path::{Path, PathBuf};

use heed::types::{Bytes, Str};
use heed::{Database, Env, EnvFlags, EnvOpenOptions};

use crate::entry::{decode_many, encode_many, EntryRecord, EntryType};
use crate::error::Result;

/// The sentinel key that stores the root path metadata.
const ROOT_KEY: &str = "\x00__dux_root__";

pub struct DuxDb {
    env: Env,
    db: Database<Str, Bytes>,
}

impl DuxDb {
    /// Open or create the database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        // LMDB expects a directory — create it if it doesn't exist.
        fs::create_dir_all(path)?;

        let env = unsafe {
            let mut opts = EnvOpenOptions::new();
            opts.map_size(1024 * 1024 * 1024); // 1 GiB
            opts.max_dbs(1);
            // WRITE_MAP: use a writable mmap, faster than default write-through.
            // MAP_ASYNC: don't flush pages to disk synchronously on commit.
            // Safe for dux since the DB is always re-generable via `dux index`.
            opts.flags(EnvFlags::WRITE_MAP | EnvFlags::MAP_ASYNC);
            opts.open(path)?
        };

        let mut wtxn = env.write_txn()?;
        let db: Database<Str, Bytes> = env.create_database(&mut wtxn, Some("dirs"))?;
        wtxn.commit()?;

        Ok(DuxDb { env, db })
    }

    /// Default database path: ~/.dux.db
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".dux.db")
    }

    /// Store the root sentinel — records which path was indexed.
    pub fn put_root(&self, root_path: &str) -> Result<()> {
        let sentinel = EntryRecord {
            entry_type: EntryType::Dir,
            mtime: 0,
            size_apparent: 0,
            size_actual: 0,
            name: root_path.to_string(),
        };
        let encoded = encode_many(&[sentinel]);
        let mut wtxn = self.env.write_txn()?;
        self.db.put(&mut wtxn, ROOT_KEY, &encoded)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Read the indexed root path from the sentinel key.
    pub fn get_root(&self) -> Result<Option<String>> {
        let rtxn = self.env.read_txn()?;
        match self.db.get(&rtxn, ROOT_KEY)? {
            Some(data) => {
                let entries = decode_many(data)?;
                if let Some(e) = entries.first() {
                    Ok(Some(e.name.clone()))
                } else {
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }

    /// Read entries within an existing read transaction (avoids per-call txn overhead).
    pub fn get_dir_in_txn(
        &self,
        rtxn: &heed::RoTxn<'_>,
        dir_path: &str,
    ) -> Result<Option<Vec<EntryRecord>>> {
        match self.db.get(rtxn, dir_path)? {
            Some(data) => Ok(Some(decode_many(data)?)),
            None => Ok(None),
        }
    }

    /// Begin a read transaction.
    pub fn read_txn(&self) -> Result<heed::RoTxn<'_>> {
        Ok(self.env.read_txn()?)
    }

    /// Batch-write multiple directories in a single transaction.
    /// Merges with any existing entries for the same key to avoid data loss
    /// when a directory's entries are split across multiple flush batches.
    pub fn put_batch_merge<'a>(
        &self,
        dirs: impl Iterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<()> {
        let mut wtxn = self.env.write_txn()?;
        for (path, new_encoded) in dirs {
            if let Some(existing) = self.db.get(&wtxn, path)? {
                // O(1) merge: our binary format is a headerless sequence of entries,
                // so concatenating raw bytes is identical to decode+extend+encode.
                let mut merged = Vec::with_capacity(existing.len() + new_encoded.len());
                merged.extend_from_slice(existing);
                merged.extend_from_slice(new_encoded);
                self.db.put(&mut wtxn, path, &merged)?;
            } else {
                self.db.put(&mut wtxn, path, new_encoded)?;
            }
        }
        wtxn.commit()?;
        Ok(())
    }

    /// Wipe all entries from the database.
    pub fn clear(&self) -> Result<()> {
        let mut wtxn = self.env.write_txn()?;
        self.db.clear(&mut wtxn)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Number of keys in the database.
    pub fn entry_count(&self) -> Result<u64> {
        let rtxn = self.env.read_txn()?;
        Ok(self.db.len(&rtxn)?)
    }

    /// Actual data size in the database (non-free pages only).
    /// With WRITE_MAP the file is pre-allocated to map_size, so file size is misleading.
    pub fn disk_size(&self) -> Result<u64> {
        Ok(self.env.non_free_pages_size()?)
    }

}
