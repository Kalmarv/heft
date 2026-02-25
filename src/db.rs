#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use heed::types::{Bytes, Str};
use heed::{Database, Env, EnvOpenOptions};

use crate::entry::{decode_many, encode_many, EntryRecord, EntryType};
use crate::error::Result;

/// The sentinel key that stores the root path metadata.
const ROOT_KEY: &str = "\x00__dux_root__";

pub struct DuxDb {
    env: Env,
    db: Database<Str, Bytes>,
    path: PathBuf,
}

impl DuxDb {
    /// Open or create the database at the given path.
    pub fn open(path: &Path) -> Result<Self> {
        // LMDB expects a directory — create it if it doesn't exist.
        fs::create_dir_all(path)?;

        let env = unsafe {
            EnvOpenOptions::new()
                .map_size(1024 * 1024 * 1024) // 1 GiB
                .max_dbs(1)
                .open(path)?
        };

        let mut wtxn = env.write_txn()?;
        let db: Database<Str, Bytes> = env.create_database(&mut wtxn, Some("dirs"))?;
        wtxn.commit()?;

        Ok(DuxDb {
            env,
            db,
            path: path.to_path_buf(),
        })
    }

    /// Default database path: ~/.dux.db
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".dux.db")
    }

    /// Store encoded entries for a directory path.
    pub fn put_dir(&self, dir_path: &str, entries: &[EntryRecord]) -> Result<()> {
        let encoded = encode_many(entries);
        let mut wtxn = self.env.write_txn()?;
        self.db.put(&mut wtxn, dir_path, &encoded)?;
        wtxn.commit()?;
        Ok(())
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

    /// Read entries for a directory.
    pub fn get_dir(&self, dir_path: &str) -> Result<Option<Vec<EntryRecord>>> {
        let rtxn = self.env.read_txn()?;
        match self.db.get(&rtxn, dir_path)? {
            Some(data) => Ok(Some(decode_many(data)?)),
            None => Ok(None),
        }
    }

    /// Batch-write multiple directories in a single transaction.
    pub fn put_batch<'a>(
        &self,
        dirs: impl Iterator<Item = (&'a str, Vec<u8>)>,
    ) -> Result<()> {
        let mut wtxn = self.env.write_txn()?;
        for (path, encoded) in dirs {
            self.db.put(&mut wtxn, path, &encoded)?;
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

    /// Size of the database file on disk.
    pub fn disk_size(&self) -> Result<u64> {
        // LMDB stores data in a single `data.mdb` file inside the directory.
        let mdb_path = self.path.join("data.mdb");
        match fs::metadata(&mdb_path) {
            Ok(m) => Ok(m.len()),
            Err(_) => Ok(0),
        }
    }

    /// Iterate all key-value pairs in the database.
    pub fn iter_all(&self) -> Result<Vec<(String, Vec<EntryRecord>)>> {
        let rtxn = self.env.read_txn()?;
        let mut result = Vec::new();
        let iter = self.db.iter(&rtxn)?;
        for item in iter {
            let (key, val) = item?;
            if key == ROOT_KEY {
                continue;
            }
            let entries = decode_many(val)?;
            result.push((key.to_string(), entries));
        }
        Ok(result)
    }

    /// Get a reference to the environment (for advanced use).
    pub fn env(&self) -> &Env {
        &self.env
    }
}
