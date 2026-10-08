//! The two engines must be indistinguishable from outside.
//!
//! This is the test that matters most in this change. A vector path that
//! diverges does not crash and does not fail a unit test on field arithmetic:
//! it produces valid-looking keys attached to the wrong counter offsets, and
//! the failure surfaces when a user cannot open the address they were given.

use onion_gen::batch::BatchEngine;
use onion_gen::batch4::BatchEngine4;
use onion_gen::{curve, field4, key, run};

/// Enough candidates that a rare lane or carry mistake has to show up.
const CANDIDATES: usize = 120_000;

fn start(seed_index: u64) -> curve::Point {
    let seed = run::block_seed(&[0x5a; 32], seed_index);
    let scalar = key::secret_scalar(&seed);
    curve::scalar_base_mult(&scalar, &curve::Point::basepoint(), &curve::two_d())
}

#[test]
fn both_engines_produce_identical_candidates() {
    if !run::uses_vector_path() {
        eprintln!(
            "skipped: no vector field implementation on this target ({})",
            field4::describe()
        );
        return;
    }
    eprintln!("comparing against: {}", field4::describe());

    const BATCH: usize = 2048;
    let mut scalar_engine = BatchEngine::new(BATCH);
    let mut vector_engine = BatchEngine4::new(BATCH);

    let mut compared = 0usize;
    let mut block = 0u64;
    while compared < CANDIDATES {
        let mut scalar_acc = start(block);
        let mut chains = BatchEngine4::split_chains(&start(block));
        block += 1;

        for round in 0..4 {
            scalar_engine.run(&mut scalar_acc);
            vector_engine.run(&mut chains);

            for i in 0..BATCH {
                assert_eq!(
                    vector_engine.packed_with_sign(i),
                    scalar_engine.packed_with_sign(i),
                    "block {block} round {round} candidate {i}"
                );
            }
            compared += BATCH;
        }
    }
    eprintln!("compared {compared} candidates, byte for byte");
}

/// The addresses, not just the packed keys — the checksum and base32 encoding
/// sit between the two and a difference there would be just as invisible.
#[test]
fn both_engines_produce_identical_addresses() {
    if !run::uses_vector_path() {
        eprintln!("skipped: no vector field implementation on this target");
        return;
    }

    const BATCH: usize = 512;
    let mut scalar_acc = start(11);
    let mut chains = BatchEngine4::split_chains(&start(11));
    let mut scalar_engine = BatchEngine::new(BATCH);
    let mut vector_engine = BatchEngine4::new(BATCH);

    scalar_engine.run(&mut scalar_acc);
    vector_engine.run(&mut chains);

    for i in 0..BATCH {
        let from_scalar = key::address(&scalar_engine.packed_with_sign(i));
        let from_vector = key::address(&vector_engine.packed_with_sign(i));
        assert_eq!(from_scalar, from_vector, "candidate {i}");
    }
}
