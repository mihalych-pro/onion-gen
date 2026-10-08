//! The MySQL schema, one step per version.
//!
//! Versions are a single sequence across all three databases, so that
//! "schema 1" names the same shape wherever it is; the statements differ
//! because the dialects do.
//!
//! Nothing here ever drops or rewrites. A column that stops being used is
//! left where it is: a migration that destroys data cannot be undone, and
//! what this table holds cannot be regenerated.

use crate::store::sql::Migration;

pub const ALL: &[Migration] = &[Migration {
    version: 1,
    name: "the keys table",
    statements: &[
        // `utf8mb4_bin` rather than the default collation: an address is
        // base32 and two addresses differing only in case are two addresses.
        // Under a case-insensitive collation the primary key would merge them.
        "CREATE TABLE IF NOT EXISTS onion_keys (
             address       VARCHAR(70) NOT NULL PRIMARY KEY,
             secret_key    TEXT NOT NULL,
             public_key    TEXT NOT NULL,
             filter        TEXT NOT NULL,
             score         DOUBLE NOT NULL,
             found_at      BIGINT NOT NULL,
             block         BIGINT NOT NULL,
             scalar_offset BIGINT NOT NULL,
             source        VARCHAR(64) NOT NULL,
             INDEX onion_keys_by_score (score DESC),
             INDEX onion_keys_by_found_at (found_at)
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin",
    ],
}];
