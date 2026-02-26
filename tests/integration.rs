use std::fs;
use std::path::PathBuf;

/// Create a unique temp directory for each test's LMDB database.
fn temp_db_path(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!("heft-test-{}-{}", name, std::process::id()));
    let _ = fs::remove_dir_all(&path);
    path
}

/// Clean up a temp DB directory.
fn cleanup(path: &PathBuf) {
    let _ = fs::remove_dir_all(path);
}

mod db_tests {
    use super::*;

    #[test]
    fn open_and_root_sentinel() {
        let db_path = temp_db_path("root-sentinel");
        let db = heft::db::HeftDb::open(&db_path, 1).unwrap();

        // No root initially.
        assert!(db.get_root().unwrap().is_none());

        // Store and retrieve root.
        db.put_root("/home/user/data").unwrap();
        assert_eq!(db.get_root().unwrap().unwrap(), "/home/user/data");

        // Overwriting root works.
        db.put_root("/mnt/storage").unwrap();
        assert_eq!(db.get_root().unwrap().unwrap(), "/mnt/storage");

        cleanup(&db_path);
    }

    #[test]
    fn clear_wipes_everything() {
        let db_path = temp_db_path("clear");
        let db = heft::db::HeftDb::open(&db_path, 1).unwrap();

        db.put_root("/test").unwrap();
        let entries = heft::entry::encode_many(&[heft::entry::EntryRecord {
            entry_type: heft::entry::EntryType::File,
            mtime: 1,
            size_apparent: 1,
            size_actual: 1,
            name: "x".into(),
        }]);
        {
            let mut wtxn = db.write_txn().unwrap();
            db.put_dir_in_txn(&mut wtxn, "/test", &entries).unwrap();
            wtxn.commit().unwrap();
        }

        let rtxn = db.read_txn().unwrap();
        assert!(db.get_dir_in_txn(&rtxn, "/test").unwrap().is_some());
        drop(rtxn);

        db.clear().unwrap();
        assert!(db.get_root().unwrap().is_none());
        let rtxn = db.read_txn().unwrap();
        assert!(db.get_dir_in_txn(&rtxn, "/test").unwrap().is_none());

        cleanup(&db_path);
    }

    #[test]
    fn disk_size() {
        let db_path = temp_db_path("stats");
        let db = heft::db::HeftDb::open(&db_path, 1).unwrap();

        db.put_root("/test").unwrap();
        let entries = heft::entry::encode_many(&[heft::entry::EntryRecord {
            entry_type: heft::entry::EntryType::Dir,
            mtime: 0,
            size_apparent: 0,
            size_actual: 4096,
            name: "sub".into(),
        }]);
        {
            let mut wtxn = db.write_txn().unwrap();
            db.put_dir_in_txn(&mut wtxn, "/test", &entries).unwrap();
            wtxn.commit().unwrap();
        }

        assert!(db.disk_size().unwrap() > 0);

        cleanup(&db_path);
    }
}

mod tree_tests {
    use super::*;

    /// Helper: populate a DB with a known directory structure and return the DB.
    ///
    /// Structure:
    ///   /root
    ///     ├── docs/
    ///     │   ├── readme.txt   (apparent=100, actual=4096, mtime=500)
    ///     │   └── notes.txt    (apparent=50,  actual=4096, mtime=600)
    ///     ├── src/
    ///     │   └── main.rs      (apparent=200, actual=4096, mtime=400)
    ///     └── photo.jpg        (apparent=3000, actual=4096, mtime=300)
    fn setup_test_tree(name: &str) -> (PathBuf, heft::db::HeftDb) {
        let db_path = temp_db_path(name);
        let db = heft::db::HeftDb::open(&db_path, 1).unwrap();

        db.put_root("/root").unwrap();

        // /root contains: docs/, src/, photo.jpg
        let root_entries = heft::entry::encode_many(&[
            heft::entry::EntryRecord {
                entry_type: heft::entry::EntryType::Dir,
                mtime: 0,
                size_apparent: 0,
                size_actual: 0,
                name: "docs".into(),
            },
            heft::entry::EntryRecord {
                entry_type: heft::entry::EntryType::Dir,
                mtime: 0,
                size_apparent: 0,
                size_actual: 0,
                name: "src".into(),
            },
            heft::entry::EntryRecord {
                entry_type: heft::entry::EntryType::File,
                mtime: 300,
                size_apparent: 3000,
                size_actual: 4096,
                name: "photo.jpg".into(),
            },
        ]);

        // /root/docs contains: readme.txt, notes.txt
        let docs_entries = heft::entry::encode_many(&[
            heft::entry::EntryRecord {
                entry_type: heft::entry::EntryType::File,
                mtime: 500,
                size_apparent: 100,
                size_actual: 4096,
                name: "readme.txt".into(),
            },
            heft::entry::EntryRecord {
                entry_type: heft::entry::EntryType::File,
                mtime: 600,
                size_apparent: 50,
                size_actual: 4096,
                name: "notes.txt".into(),
            },
        ]);

        // /root/src contains: main.rs
        let src_entries = heft::entry::encode_many(&[heft::entry::EntryRecord {
            entry_type: heft::entry::EntryType::File,
            mtime: 400,
            size_apparent: 200,
            size_actual: 4096,
            name: "main.rs".into(),
        }]);

        {
            let mut wtxn = db.write_txn().unwrap();
            db.put_dir_in_txn(&mut wtxn, "/root", &root_entries).unwrap();
            db.put_dir_in_txn(&mut wtxn, "/root/docs", &docs_entries).unwrap();
            db.put_dir_in_txn(&mut wtxn, "/root/src", &src_entries).unwrap();
            wtxn.commit().unwrap();
        }

        (db_path, db)
    }

    #[test]
    fn tree_aggregate_sizes() {
        let (db_path, db) = setup_test_tree("tree-agg");

        let tree = heft::tree::build_tree(&db, "/root").unwrap();

        // Total apparent: 100 + 50 + 200 + 3000 = 3350
        assert_eq!(tree.size_apparent, 3350);
        // Total actual: 4096 * 4 = 16384
        assert_eq!(tree.size_actual, 16384);
        // Max mtime across all descendants: 600
        assert_eq!(tree.mtime, 600);
        // Count: 4 files + 2 subdirs + 1 root = 7
        assert_eq!(tree.count, 7);

        cleanup(&db_path);
    }

    #[test]
    fn tree_children_sorted_by_size_desc() {
        let (db_path, db) = setup_test_tree("tree-sort");

        let tree = heft::tree::build_tree(&db, "/root").unwrap();
        let children = tree.children.as_ref().unwrap();

        // Children should be sorted by size_actual descending.
        for pair in children.windows(2) {
            assert!(
                pair[0].size_actual >= pair[1].size_actual,
                "children not sorted: {} ({}) before {} ({})",
                pair[0].name, pair[0].size_actual,
                pair[1].name, pair[1].size_actual
            );
        }

        cleanup(&db_path);
    }

    #[test]
    fn tree_subdirectory_aggregation() {
        let (db_path, db) = setup_test_tree("tree-subdir");

        // Build tree for just the docs subdirectory.
        let docs_tree = heft::tree::build_tree(&db, "/root/docs").unwrap();

        assert_eq!(docs_tree.name, "docs");
        assert_eq!(docs_tree.size_apparent, 150); // 100 + 50
        assert_eq!(docs_tree.size_actual, 8192); // 4096 + 4096
        assert_eq!(docs_tree.mtime, 600); // max(500, 600)
        assert_eq!(docs_tree.count, 3); // 2 files + 1 dir

        cleanup(&db_path);
    }

    #[test]
    fn aggregate_sizes_matches_tree() {
        let (db_path, db) = setup_test_tree("tree-agg-match");

        // build_tree opens its own read txn internally, so call it first
        // and capture results before opening another txn for aggregate_sizes.
        // (LMDB allows only one read txn per thread.)
        let tree = heft::tree::build_tree(&db, "/root").unwrap();

        let rtxn = db.read_txn().unwrap();
        let agg = heft::tree::aggregate_sizes(&db, &rtxn, "/root").unwrap();

        assert_eq!(agg.size_apparent, tree.size_apparent);
        assert_eq!(agg.size_actual, tree.size_actual);
        assert_eq!(agg.mtime, tree.mtime);
        assert_eq!(agg.count, tree.count);

        // 4 files: readme.txt, notes.txt, main.rs, photo.jpg
        assert_eq!(agg.file_count, 4);
        // 3 dirs: root, docs, src
        assert_eq!(agg.dir_count, 3);

        cleanup(&db_path);
    }

    #[test]
    fn empty_directory_produces_leaf_node() {
        let db_path = temp_db_path("tree-empty");
        let db = heft::db::HeftDb::open(&db_path, 1).unwrap();

        db.put_root("/empty").unwrap();
        // No entries written for /empty — it's an empty directory.

        let tree = heft::tree::build_tree(&db, "/empty").unwrap();
        assert_eq!(tree.size_apparent, 0);
        assert_eq!(tree.size_actual, 0);
        assert_eq!(tree.count, 1); // just the dir itself
        assert!(tree.children.is_none());

        cleanup(&db_path);
    }
}
