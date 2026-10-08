//! Keys in a SQLite file.
//!
//! The default, and the only one that needs nothing installed.

mod migrations;

use super::backend::{Backend, Stats};
use super::sql::{self, Counters, Row, LEDGER, TABLE};
use super::Find;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteSynchronous};
use sqlx::AssertSqlSafe;
use std::io;
use std::path::Path;
use std::str::FromStr;
use tokio::runtime::Runtime;

/// Rows per statement. SQLite takes 32766 parameters by default, so nine
/// columns leave room for far more than this; the limit is here so that the
/// three backends fail the same way rather than two of them differently.
const CHUNK: usize = 500;

pub struct Sqlite {
    rt: Runtime,
    pool: SqlitePool,
    shown: String,
    counters: Counters,
}

impl Sqlite {
    /// Opens the file, creating it and any missing parent directory.
    ///
    /// The file holds secret keys, so it is narrowed to its owner the moment
    /// it exists — before anything is written to it.
    pub fn open(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let fresh = !path.exists();
        let rt = sql::runtime()?;
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))
            .map_err(sql::as_io)?
            .create_if_missing(true)
            // One transaction per batch is what makes this fast, so the work
            // per find is a row rather than a directory. `Full` rather than
            // `Normal`: the batch already names the window in which a find can
            // be lost, and widening it for throughput we do not need is a poor
            // trade for a secret key.
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Full);
        let pool = sql::wait(
            &rt,
            // One connection: SQLite takes one writer, and a pool that hands
            // out more only turns a lock into an error.
            SqlitePoolOptions::new()
                .max_connections(1)
                .connect_with(options),
        )
        .map_err(sql::as_io)?;
        if fresh {
            crate::output::restrict(path)?;
        }
        Ok(Sqlite {
            rt,
            pool,
            shown: path.display().to_string(),
            counters: Counters::default(),
        })
    }
}

impl Backend for Sqlite {
    fn kind(&self) -> &'static str {
        "sqlite"
    }

    fn describe(&self) -> String {
        self.shown.clone()
    }

    fn migrate(&mut self) -> io::Result<usize> {
        sql::wait(&self.rt, async {
            // Asked first, for the same reason the server backends do it: one
            // shape of this function, so a change to it cannot be made in two
            // places and forgotten in the third.
            let ledger = sqlx::query(AssertSqlSafe(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?".to_string(),
            ))
            .bind(LEDGER)
            .fetch_optional(&self.pool)
            .await?
            .is_some();
            if !ledger {
                sqlx::query(AssertSqlSafe(format!(
                    "CREATE TABLE IF NOT EXISTS {LEDGER} (
                         version    INTEGER NOT NULL PRIMARY KEY,
                         name       VARCHAR(128) NOT NULL,
                         applied_at INTEGER NOT NULL
                     )"
                )))
                .execute(&self.pool)
                .await?;
            }

            let mut applied = 0;
            for step in migrations::ALL {
                let mut tx = self.pool.begin().await?;
                let done = sqlx::query(AssertSqlSafe(format!(
                    "SELECT version FROM {LEDGER} WHERE version = ?"
                )))
                .bind(step.version)
                .fetch_optional(&mut *tx)
                .await?
                .is_some();
                if done {
                    continue;
                }
                for statement in step.statements {
                    sqlx::query(AssertSqlSafe(statement.to_string()))
                        .execute(&mut *tx)
                        .await?;
                }
                sqlx::query(AssertSqlSafe(format!(
                    "INSERT INTO {LEDGER} (version, name, applied_at) VALUES (?, ?, ?)"
                )))
                .bind(step.version)
                .bind(step.name)
                .bind(sql::now())
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                applied += 1;
            }
            Ok::<_, sqlx::Error>(applied)
        })
        .map_err(sql::as_io)
    }

    fn holds(&mut self, address: &str) -> io::Result<bool> {
        sql::wait(&self.rt, async {
            let found = sqlx::query(AssertSqlSafe(format!(
                "SELECT 1 FROM {TABLE} WHERE address = ?"
            )))
            .bind(address)
            .fetch_optional(&self.pool)
            .await?;
            Ok::<_, sqlx::Error>(found.is_some())
        })
        .map_err(sql::as_io)
    }

    fn write(&mut self, batch: &[Find]) -> io::Result<()> {
        let now = sql::now();
        let outcome = sql::wait(&self.rt, async {
            let mut tx = self.pool.begin().await?;
            for chunk in batch.chunks(CHUNK) {
                let values = vec!["(?, ?, ?, ?, ?, ?, ?, ?, ?)"; chunk.len()].join(", ");
                let mut query = sqlx::query(AssertSqlSafe(format!(
                    "INSERT OR IGNORE INTO {TABLE}
                     (address, secret_key, public_key, filter, score,
                      found_at, block, scalar_offset, source)
                     VALUES {values}"
                )));
                for find in chunk {
                    let row = Row::of(find, now);
                    query = query
                        .bind(row.address)
                        .bind(row.secret_key)
                        .bind(row.public_key)
                        .bind(row.filter)
                        .bind(row.score)
                        .bind(row.found_at)
                        .bind(row.block)
                        .bind(row.scalar_offset)
                        .bind(row.source);
                }
                query.execute(&mut *tx).await?;
            }
            tx.commit().await
        });
        match outcome {
            Ok(()) => {
                self.counters
                    .committed(batch.len() as u64, sql::weight(batch));
                Ok(())
            }
            Err(e) => {
                self.counters.failed();
                Err(sql::as_io(e))
            }
        }
    }

    fn stats(&self) -> Stats {
        self.counters
            .read_into(self.pool.size(), self.pool.num_idle() as u32)
    }
}
