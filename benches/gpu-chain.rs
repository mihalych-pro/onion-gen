//! Does the device path in the product find the same addresses as the
//! processor, on every API this machine offers?
//!
//! One kernel reaching Vulkan, Metal and DX12 goes through three different
//! compilers, and the vendor path through a fourth. Agreement on one of them
//! says nothing about the others, so this runs every device the machine has.
//!
//! Run with: cargo run --release --example gpu-chain

use onion_gen::curve::{self, Point};
use onion_gen::filter::FilterSet;
use onion_gen::gpu;
use onion_gen::key;

fn main() {
    // Both paths, without the de-duplication `Want::Any` does: a card reachable
    // two ways has to be checked both ways, and on the one machine where that
    // happens the automatic choice would otherwise hide the vendor path.
    let mut devices = Vec::new();
    for want in [
        gpu::Want::Portable(None),
        gpu::Want::Vendor,
        gpu::Want::OpenCl,
    ] {
        match gpu::look(want) {
            gpu::Availability::Devices(d) => devices.extend(d),
            other => println!("{want:?}: nothing to run on ({other:?})"),
        }
    }
    if devices.is_empty() {
        println!("no device to run on");
        return;
    }

    let set = FilterSet::parse_all(["abc"]).expect("a valid filter");
    let (words, bits) = set.index_words().expect("a prefix set has a bitmap");

    let seed = [7u8; 32];
    let scalar = key::secret_scalar(&seed);
    let start = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
    let threads = 1u32 << 14;

    let mut failed = 0usize;
    for device in &devices {
        println!("device: {device}");
        let mut engine = match gpu::Engine::new(device, &start, threads, words, bits, false) {
            Ok(e) => e,
            Err(e) => {
                println!("  could not start: {e}");
                failed += 1;
                continue;
            }
        };

        let mut examined = 0u64;
        let mut survivors = 0usize;
        let mut confirmed = 0usize;
        for _ in 0..4 {
            let hits = engine.advance().expect("launch");
            examined += engine.batch();
            survivors += hits.len();
            for hit in &hits {
                // The processor recomputes the key from the offset alone. If
                // the device got the bookkeeping wrong, these will not match.
                let secret = key::expanded_secret_at_offset(&seed, hit.offset).expect("in range");
                let mut sc = [0u8; 32];
                sc.copy_from_slice(&secret[..32]);
                let p = curve::scalar_base_mult(&sc, &Point::basepoint(), &curve::two_d());
                let mut want = key::pack(&p);
                // The device does not set the sign bit; the comparison is of
                // the coordinate only.
                want[31] &= 0x7f;
                assert_eq!(
                    want, hit.packed,
                    "{device} and the processor disagree at offset {}",
                    hit.offset
                );
                if set.match_key(&hit.packed).is_some() {
                    confirmed += 1;
                }
            }
        }
        println!(
            "  examined {examined}, {survivors} survived the bitmap, {confirmed} match exactly"
        );
        assert!(
            survivors > 0,
            "{device} let nothing through a 3-symbol index"
        );
    }
    if failed > 0 {
        std::process::exit(1);
    }
}
