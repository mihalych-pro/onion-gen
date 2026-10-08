//! Keys in MySQL.

mod migrations;

use super::backend::{Backend, Stats};
use super::sql::{self, Counters, Row, LEDGER, TABLE};
use super::Find;
use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::AssertSqlSafe;
use std::io;
use tokio::runtime::Runtime;

/// Rows per statement. Bounded by `max_allowed_packet` rather than by a
/// parameter count; 500 rows of around 300 bytes stays far inside the 4 MiB
/// a stock server allows.
const CHUNK: usize = 500;

pub struct MySql {
    rt: Runtime,
    pool: MySqlPool,
    shown: String,
    counters: Counters,
}

impl MySql {
    pub fn open(url: &str) -> io::Result<Self> {
        let rt = sql::runtime()?;
        let pool = sql::wait(&rt, MySqlPoolOptions::new().max_connections(2).connect(url))
            .map_err(sql::as_io)?;
        Ok(MySql {
            rt,
            pool,
            shown: sql::redact(url),
            counters: Counters::default(),
        })
    }
}

impl Backend for MySql {
    fn kind(&self) -> &'static str {
        "mysql"
    }

    fn describe(&self) -> String {
        self.shown.clone()
    }

    fn migrate(&mut self) -> io::Result<usize> {
        sql::wait(&self.rt, async {
            // Asked rather than created unconditionally, so that an account
            // granted only SELECT and INSERT can still start. See the note in
            // the PostgreSQL backend.
            let ledger = sqlx::query(AssertSqlSafe(
                "SELECT 1 FROM information_schema.tables
                 WHERE table_schema = DATABASE() AND table_name = ?"
                    .to_string(),
            ))
            .bind(LEDGER)
            .fetch_optional(&self.pool)
            .await?
            .is_some();
            if !ledger {
                sqlx::query(AssertSqlSafe(format!(
                    "CREATE TABLE IF NOT EXISTS {LEDGER} (
                         version    BIGINT NOT NULL PRIMARY KEY,
                         name       VARCHAR(128) NOT NULL,
                         applied_at BIGINT NOT NULL
                     ) ENGINE=InnoDB"
                )))
                .execute(&self.pool)
                .await?;
            }

            let mut applied = 0;
            for step in migrations::ALL {
                // DDL is not transactional here — MySQL commits implicitly at
                // every CREATE — so the ledger is written last. A failure part
                // way leaves the step unrecorded and it runs again, which is
                // why every statement above is `IF NOT EXISTS`.
                let done = sqlx::query(AssertSqlSafe(format!(
                    "SELECT version FROM {LEDGER} WHERE version = ?"
                )))
                .bind(step.version)
                .fetch_optional(&self.pool)
                .await?
                .is_some();
                if done {
                    continue;
                }
                for statement in step.statements {
                    sqlx::query(AssertSqlSafe(statement.to_string()))
                        .execute(&self.pool)
                        .await?;
                }
                sqlx::query(AssertSqlSafe(format!(
                    "INSERT INTO {LEDGER} (version, name, applied_at) VALUES (?, ?, ?)"
                )))
                .bind(step.version)
                .bind(step.name)
                .bind(sql::now())
                .execute(&self.pool)
                .await?;
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
                    "INSERT IGNORE INTO {TABLE}
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
