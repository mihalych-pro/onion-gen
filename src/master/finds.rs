//! Turning a reported position into a key, and refusing it when it is not one.
//!
//! The worker says where it found something; the master, which handed out the
//! seed, works out what. Deriving the key also checks it: a broken or
//! misconfigured worker cannot put an address into the store that the master
//! does not reach from the position it was given.
//!
//! Everything refused here is counted and shown in the fleet view, so that a
//! worker failing every find does not look like a worker having no luck.

use crate::filter::FilterSet;
use crate::key;
use crate::run::block_seed;

/// What the master made of a position a worker reported.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// The position yields a key whose address satisfies a filter.
    Good {
        address: String,
        public_key: [u8; 32],
        secret: Box<[u8; 64]>,
        filter: String,
    },
    /// The position is outside the range the worker is holding. A worker that
    /// has a lease and reports from beyond it is misbehaving, and that is
    /// worth refusing and counting; a worker whose lease has simply gone is a
    /// different case and is not refused.
    OutsideLease,
    /// The position yields a key, but its address satisfies nothing that was
    /// asked for.
    NoMatch { address: String },
    /// The position yields no usable key at all. Clamping rules a small share
    /// of offsets out, and a single run drops those silently; here it means the
    /// worker reported one it should itself have dropped.
    NotAKey,
}

/// Works out what a worker's report amounts to.
///
/// `root` is the seed the master handed out, `block` and `offset` the position
/// reported. The lease's range is checked first, before any arithmetic: the
/// cheapest refusal is the one that needs no work.
pub fn examine(
    root: &[u8; 32],
    lease_range: Option<super::Range>,
    block: u64,
    offset: u64,
    filters: &FilterSet,
) -> Verdict {
    // `None` means there is no lease to check against — it expired, or the
    // master was restarted and lost the one it had handed out. The find is
    // still checked, just by what actually matters: whether the position
    // yields an address somebody asked for.
    //
    // Refusing it instead would throw away a key over bookkeeping. A worker
    // holding finds through a master restart hands them over as designed, and
    // the master would reject every one — the buffer that exists so a week's
    // work is not lost would be handing that work to something that discards
    // it.
    //
    // Nothing is given away by this. Producing a position that derives to a
    // matching address requires the seed, and whoever has the seed can search
    // for keys without asking the master at all.
    if lease_range.is_some_and(|range| !range.contains(block)) {
        return Verdict::OutsideLease;
    }
    let seed = block_seed(root, block);
    let Some(secret) = key::expanded_secret_at_offset(&seed, offset) else {
        return Verdict::NotAKey;
    };
    let public_key = key::public_key_from_secret(&secret);
    let address = key::address(&public_key);
    match filters.match_address_text(&address) {
        Some(filter) => Verdict::Good {
            address,
            public_key,
            secret: Box::new(secret),
            filter: filter.source_text().to_string(),
        },
        None => Verdict::NoMatch { address },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{BatchEngine, CHAIN_STEP};
    use crate::curve::{self, Point};
    use crate::master::Range;

    /// Searches one block the way a worker does, and returns the first hit as
    /// the position it would report.
    fn find_in_block(root: &[u8; 32], block: u64, prefix: &str) -> (u64, String) {
        let seed = block_seed(root, block);
        let scalar = key::secret_scalar(&seed);
        let mut acc = curve::scalar_base_mult(&scalar, &Point::basepoint(), &curve::two_d());
        let mut engine = BatchEngine::new(1024);
        let mut base = 0u64;
        for _ in 0..512 {
            engine.run(&mut acc);
            for i in 0..engine.batch_size() {
                let packed = engine.packed_with_sign(i);
                let address = key::address(&packed);
                if address.starts_with(prefix) {
                    return (base + CHAIN_STEP * i as u64, address);
                }
            }
            base += CHAIN_STEP * engine.batch_size() as u64;
        }
        panic!("no hit for {prefix} in block {block}");
    }

    /// The claim the whole arrangement rests on: what the worker found is what
    /// the master reconstructs from two numbers.
    #[test]
    fn the_master_arrives_at_the_address_the_worker_found() {
        let root = [0x2fu8; 32];
        let filters = FilterSet::parse_all(["ab"]).expect("a valid filter");
        let range = Range {
            first: 0,
            blocks: 8,
        };

        for block in 0..3u64 {
            let (offset, found) = find_in_block(&root, block, "ab");
            match examine(&root, Some(range), block, offset, &filters) {
                Verdict::Good {
                    address, secret, ..
                } => {
                    assert_eq!(address, found, "block {block}");
                    // And the key really opens that address, checked the same
                    // way a single run checks its own finds.
                    let public = key::public_key_from_secret(&secret);
                    assert_eq!(key::address(&public), found);
                }
                other => panic!("block {block}: {other:?}"),
            }
        }
    }

    #[test]
    fn a_position_outside_the_lease_is_refused_before_any_work() {
        let root = [0x2fu8; 32];
        let filters = FilterSet::parse_all(["ab"]).expect("a valid filter");
        let (offset, _) = find_in_block(&root, 5, "ab");
        // A real find, reported against a lease that does not cover it.
        let verdict = examine(
            &root,
            Some(Range {
                first: 0,
                blocks: 3,
            }),
            5,
            offset,
            &filters,
        );
        assert_eq!(verdict, Verdict::OutsideLease);
    }

    #[test]
    fn a_position_that_matches_nothing_is_refused() {
        let root = [0x2fu8; 32];
        let filters = FilterSet::parse_all(["zzzz"]).expect("a valid filter");
        let (offset, found) = find_in_block(&root, 0, "ab");
        match examine(
            &root,
            Some(Range {
                first: 0,
                blocks: 8,
            }),
            0,
            offset,
            &filters,
        ) {
            Verdict::NoMatch { address } => assert_eq!(address, found),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    /// A worker reporting a position from a different seed cannot smuggle an
    /// address in: the master derives from its own seed and arrives elsewhere.
    /// A master that was restarted has no record of the lease, and the worker
    /// that held it is handing over exactly what it was supposed to keep.
    #[test]
    fn a_find_without_a_lease_is_still_checked_rather_than_refused() {
        let root = [0x2fu8; 32];
        let filters = FilterSet::parse_all(["ab"]).expect("a valid filter");
        let (offset, found) = find_in_block(&root, 2, "ab");

        match examine(&root, None, 2, offset, &filters) {
            Verdict::Good { address, .. } => assert_eq!(address, found),
            other => panic!("a good key must not be thrown away: {other:?}"),
        }
        // And one that matches nothing is still refused without a lease.
        let strict = FilterSet::parse_all(["zzzz"]).expect("a valid filter");
        assert!(matches!(
            examine(&root, None, 2, offset, &strict),
            Verdict::NoMatch { .. }
        ));
    }

    #[test]
    fn a_position_from_another_seed_does_not_carry_its_address() {
        let theirs = [0x77u8; 32];
        let ours = [0x2fu8; 32];
        let filters = FilterSet::parse_all(["ab"]).expect("a valid filter");
        let (offset, found) = find_in_block(&theirs, 0, "ab");

        match examine(
            &ours,
            Some(Range {
                first: 0,
                blocks: 8,
            }),
            0,
            offset,
            &filters,
        ) {
            Verdict::Good { address, .. } => {
                assert_ne!(address, found, "the master must not reach their address")
            }
            Verdict::NoMatch { address } => assert_ne!(address, found),
            Verdict::NotAKey => {}
            other => panic!("unexpected {other:?}"),
        }
    }
}
