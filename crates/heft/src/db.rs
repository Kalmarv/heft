use std::fs;
use std::path::{Path, PathBuf};

use heed::types::{Bytes, Str};
use heed::{Database, Env, EnvFlags, EnvOpenOptions};

use crate::entry::{decode_many, encode_many, EntryRecord, EntryType};
use crate::error::Result;

/// The sentinel key that stores the root path metadata.
const ROOT_KEY: &str = "\x00__heft_root__";

pub struct HeftDb {
    env: Env,
    db: Database<Str, Bytes>,
}

impl HeftDb {
    /// Open or create the database at the given path.
    pub fn open(path: &Path, map_size_gib: u64) -> Result<Self> {
        // LMDB expects a directory — create it if it doesn't exist.
        fs::create_dir_all(path)?;

        // SAFETY: heed's `EnvOpenOptions::open` is unsafe because LMDB requires:
        //  1. The path is a valid directory — guaranteed by `fs::create_dir_all` above.
        //  2. No other process opens this env with incompatible flags — LMDB's own
        //     lock file (`lock.mdb`) prevents this, and heft is a single-user CLI tool.
        //  3. The `Env` outlives all transactions — it is stored in `HeftDb` and all
        //     transactions borrow `&self`, so Rust's borrow checker enforces this.
        let env = unsafe {
            let mut opts = EnvOpenOptions::new();
            opts.map_size((map_size_gib as usize).saturating_mul(1024 * 1024 * 1024));
            opts.max_dbs(1);
            // WRITE_MAP: use a writable mmap, faster than default write-through.
            // MAP_ASYNC: don't flush pages to disk synchronously on commit.
            // The DB is always re-generable via `heft index`, so durability is not needed.
            opts.flags(EnvFlags::WRITE_MAP | EnvFlags::MAP_ASYNC);
            opts.open(path)?
        };

        let mut wtxn = env.write_txn()?;
        let db: Database<Str, Bytes> = env.create_database(&mut wtxn, Some("dirs"))?;
        wtxn.commit()?;

        Ok(HeftDb { env, db })
    }

    /// Default database path: ~/.heft.db
    pub fn default_path() -> PathBuf {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        PathBuf::from(home).join(".heft.db")
    }

    /// Store the root sentinel — records which path was indexed.
    pub fn put_root(&self, root_path: &str) -> Result<()> {
        let sentinel = EntryRecord {
            entry_type: EntryType::Dir,
            mtime: 0,
            size_apparent: 0,
            size_actual: 0,
            nlink: 1,
            dev: 0,
            ino: 0,
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

    /// Begin a write transaction (for external batch use).
    pub fn write_txn(&self) -> Result<heed::RwTxn<'_>> {
        Ok(self.env.write_txn()?)
    }

    /// Write a directory's encoded entries in an existing write transaction.
    /// No merge — the caller guarantees the data is complete for this key.
    pub fn put_dir_in_txn(
        &self,
        wtxn: &mut heed::RwTxn<'_>,
        dir_path: &str,
        encoded: &[u8],
    ) -> Result<()> {
        self.db.put(wtxn, dir_path, encoded)?;
        Ok(())
    }

    /// Wipe all entries from the database.
    pub fn clear(&self) -> Result<()> {
        let mut wtxn = self.env.write_txn()?;
        self.db.clear(&mut wtxn)?;
        wtxn.commit()?;
        Ok(())
    }

    /// Delete all keys under `root` (the root key itself + all children).
    ///
    /// Data from other roots is preserved.  The sentinel key is unaffected
    /// because it starts with `\x00`, which sorts before any `/`-prefixed path.
    pub fn clear_subtree(&self, root: &str) -> Result<()> {
        // Use prefix_iter to collect matching keys (exact prefix match — "/home/"
        // matches "/home/user" but not "/homelab"), then delete in a write txn.
        let trimmed = root.trim_end_matches('/');
        let child_prefix = if trimmed.is_empty() {
            "/".to_string()
        } else {
            format!("{trimmed}/")
        };

        let keys: Vec<String> = {
            let rtxn = self.env.read_txn()?;
            let mut keys = Vec::new();
            if self.db.get(&rtxn, root)?.is_some() {
                keys.push(root.to_string());
            }
            for result in self.db.prefix_iter(&rtxn, &child_prefix)? {
                let (key, _) = result?;
                keys.push(key.to_string());
            }
            keys
        };

        let mut wtxn = self.env.write_txn()?;
        for key in &keys {
            self.db.delete(&mut wtxn, key)?;
        }
        wtxn.commit()?;
        Ok(())
    }

    /// Actual data size in the database (non-free pages only).
    /// With WRITE_MAP the file is pre-allocated to map_size, so file size is misleading.
    pub fn disk_size(&self) -> Result<u64> {
        Ok(self.env.non_free_pages_size()?)
    }
}
