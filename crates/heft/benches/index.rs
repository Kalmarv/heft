use std::fs;
use std::path::Path;

use criterion::{criterion_group, criterion_main, Criterion};

use heft::db::HeftDb;
use heft::scanner::{self, ScanOptions};

/// Create a deterministic directory tree for benchmarking.
///
/// Layout: 50 directories, each with 20 files = 1,000 files total.
/// Some files are hardlinked to test the dedup path.
fn create_test_tree(base: &Path) {
    for dir_idx in 0..50 {
        let dir = base.join(format!("dir_{dir_idx:03}"));
        fs::create_dir_all(&dir).unwrap();

        for file_idx in 0..20 {
            let file = dir.join(format!("file_{file_idx:03}.dat"));
            // Write deterministic content sized to exercise block accounting.
            let content = vec![0u8; 256 + (file_idx * 64)];
            fs::write(&file, &content).unwrap();
        }
    }

    // Create some hardlinks to exercise the dedup path.
    let link_source = base.join("dir_000/file_000.dat");
    for i in 0..10 {
        let link_dest = base.join(format!("dir_001/hardlink_{i:03}.dat"));
        let _ = fs::hard_link(&link_source, &link_dest);
    }
}

fn bench_index(c: &mut Criterion) {
    let tree_dir = std::env::temp_dir().join(format!("heft-bench-tree-{}", std::process::id()));
    let db_path = std::env::temp_dir().join(format!("heft-bench-db-{}", std::process::id()));

    // Create the test tree once (outside the benchmark loop).
    let _ = fs::remove_dir_all(&tree_dir);
    let _ = fs::remove_dir_all(&db_path);
    create_test_tree(&tree_dir);

    c.bench_function("index_1k_files", |b| {
        b.iter(|| {
            let _ = fs::remove_dir_all(&db_path);
            let db = HeftDb::open(&db_path, 1).unwrap();
            let opts = ScanOptions {
                verbose: false,
                threads: Some(4),
                one_file_system: false,
                exclude: vec![],
            };
            scanner::run_index(&tree_dir, &db, &opts).unwrap();
        });
    });

    // Cleanup.
    let _ = fs::remove_dir_all(&tree_dir);
    let _ = fs::remove_dir_all(&db_path);
}

criterion_group!(benches, bench_index);
criterion_main!(benches);
