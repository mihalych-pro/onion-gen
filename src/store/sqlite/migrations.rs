//! The SQLite schema, one step per version.
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
        "CREATE TABLE IF NOT EXISTS onion_keys (
             address       VARCHAR(70) NOT NULL PRIMARY KEY,
             secret_key    TEXT NOT NULL,
             public_key    TEXT NOT NULL,
             filter        TEXT NOT NULL,
             score         REAL NOT NULL,
             found_at      INTEGER NOT NULL,
             block         INTEGER NOT NULL,
             scalar_offset INTEGER NOT NULL,
             source        VARCHAR(64) NOT NULL
         )",
        "CREATE INDEX IF NOT EXISTS onion_keys_by_score ON onion_keys (score DESC)",
        "CREATE INDEX IF NOT EXISTS onion_keys_by_found_at ON onion_keys (found_at)",
    ],
}];
