//! Keys in PostgreSQL.

mod migrations;

use super::backend::{Backend, Stats};
use super::sql::{self, Counters, Row, LEDGER, TABLE};
use super::Find;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::AssertSqlSafe;
use std::io;
use tokio::runtime::Runtime;

/// Rows per statement.
///
/// A batch is a thousand finds and each row binds nine parameters; the server
/// takes 65535 per statement, so one statement would be close enough to the
/// ceiling that a tenth column would break it quietly.
const CHUNK: usize = 500;

pub struct Postgres {
    rt: Runtime,
    pool: PgPool,
    shown: String,
    counters: Counters,
}

impl Postgres {
    pub fn open(url: &str) -> io::Result<Self> {
        let rt = sql::runtime()?;
        let pool = sql::wait(
            &rt,
            PgPoolOptions::new()
                // One writer, because every find already passes through one
                // lock; more connections would idle and still be counted
                // against the server's limit.
                .max_connections(2)
                .connect(url),
        )
        .map_err(sql::as_io)?;
        Ok(Postgres {
            rt,
            pool,
            shown: sql::redact(url),
            counters: Counters::default(),
        })
    }
}

impl Backend for Postgres {
    fn kind(&self) -> &'static str {
        "postgres"
    }

    fn describe(&self) -> String {
        self.shown.clone()
    }

    fn migrate(&mut self) -> io::Result<usize> {
        sql::wait(&self.rt, async {
            // Asked rather than created unconditionally: `CREATE TABLE IF NOT
            // EXISTS` still wants CREATE on the schema, so an account granted
            // only SELECT and INSERT — the arrangement the deployment notes
            // recommend — could not start.
            let ledger = sqlx::query(AssertSqlSafe(
                "SELECT 1 FROM information_schema.tables
                 WHERE table_schema = current_schema() AND table_name = $1"
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
                     )"
                )))
                .execute(&self.pool)
                .await?;
            }

            let mut applied = 0;
            for step in migrations::ALL {
                // One transaction per step, so a failure half way leaves the
                // ledger agreeing with the schema rather than ahead of it.
                let mut tx = self.pool.begin().await?;
                let done = sqlx::query(AssertSqlSafe(format!(
                    "SELECT version FROM {LEDGER} WHERE version = $1"
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
                    "INSERT INTO {LEDGER} (version, name, applied_at) VALUES ($1, $2, $3)"
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
                "SELECT 1 FROM {TABLE} WHERE address = $1"
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
                let values = (0..chunk.len())
                    .map(|r| {
                        let base = r * 9;
                        let marks = (1..=9)
                            .map(|c| format!("${}", base + c))
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("({marks})")
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut query = sqlx::query(AssertSqlSafe(format!(
                    "INSERT INTO {TABLE}
                     (address, secret_key, public_key, filter, score,
                      found_at, block, scalar_offset, source)
                     VALUES {values}
                     ON CONFLICT (address) DO NOTHING"
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
