//! How fast finds can be written down, both ways.
//!
//! The number decides whether a fleet's find rate is worth warning about: a
//! master that cannot take what its workers report makes them queue. A key
//! directory is a directory and three files; a row is a ninth of a transaction
//! shared with 999 others.

use onion_gen::key;
use onion_gen::store::{self, Find, Store};
use std::time::Instant;

/// Enough to cross several batches, few enough to finish in a few seconds on
/// the slower of the two.
const FINDS: usize = 4000;

fn finds() -> Vec<Find> {
    (0..FINDS)
        .map(|n| {
            let seed = {
                let mut b = [0u8; 32];
                b[..8].copy_from_slice(&(n as u64).to_le_bytes());
                b
            };
            let public_key = key::public_key(&seed);
            Find {
                address: key::hostname(&public_key),
                public_key,
                secret: key::expanded_secret_key(&seed),
                filter: "contains:abc".to_string(),
                score: 1.5,
                block: n as u64,
                offset: 8,
                source: "a benchmark".to_string(),
            }
        })
        .collect()
}

fn time(label: &str, mut store: Store, batch: &[Find]) {
    let start = Instant::now();
    for find in batch {
        store.put(find.clone()).expect("put");
    }
    store.flush().expect("flush");
    let seconds = start.elapsed().as_secs_f64();
    println!(
        "{label:<22} {:8.2} finds/s   {:7.3} ms each",
        batch.len() as f64 / seconds,
        seconds / batch.len() as f64 * 1e3
    );
}

fn main() {
    let batch = finds();
    let root = std::env::temp_dir().join(format!("onion-gen-store-rate-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a place to write");

    time(
        "key directories",
        Store::directory(root.join("keys")),
        &batch,
    );

    // Every store a connection string can name. The servers are optional:
    // set ONION_GEN_TEST_POSTGRES and ONION_GEN_TEST_MYSQL to measure them,
    // and without those the file figures stand on their own.
    let mut urls = vec![("sqlite", root.join("keys.db").display().to_string())];
    for (name, var) in [
        ("postgres", "ONION_GEN_TEST_POSTGRES"),
        ("mysql", "ONION_GEN_TEST_MYSQL"),
    ] {
        if let Ok(url) = std::env::var(var) {
            urls.push((name, url));
        }
    }
    for (name, url) in urls {
        match store::open(&url) {
            Ok(store) => time(name, store, &batch),
            Err(e) => println!("{name:<22} skipped: {e}"),
        }
    }

    let _ = std::fs::remove_dir_all(&root);
}
