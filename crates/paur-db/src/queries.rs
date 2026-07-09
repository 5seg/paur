//! CRUD queries for the paur schema.
//!
//! `Db` is a thin handle around an `sqlx::SqlitePool`. All methods are
//! `async` and accept `&self` so the handle is cheaply cloneable.
//!
//! The queries intentionally do not return Result<Option<...>> for
//! "found vs not" — the caller calls `get_package_by_name` and then
//! pattern-matches, which keeps the error path reserved for genuine
//! I/O failures.

use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::Row;
use sqlx::SqlitePool;

use crate::models::{Build, BuildStatus, BuildTrigger, Package, Setting, Stream};
use crate::schema;

/// Handle to the paur SQLite database. Cheap to clone (it's just an `Arc`).
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Build a Db handle from an existing pool, running migrations.
    pub async fn from_pool(pool: SqlitePool) -> paur_core::Result<Self> {
        schema::run(&pool).await?;
        Ok(Self { pool })
    }

    /// Borrow the underlying pool. Useful for ad-hoc queries.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    // -------- time helpers --------

    /// Current Unix epoch in seconds.
    pub fn now() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    // -------- packages --------

    /// Insert a new package, or update the existing row if `name` already
    /// exists. Returns the (possibly existing) package id.
    pub async fn upsert_package(
        &self,
        name: &str,
        aur_url: &str,
        auto_rebuild: bool,
    ) -> paur_core::Result<i64> {
        let now = Self::now();
        sqlx::query(
            "INSERT INTO packages (name, aur_url, added_at, auto_rebuild)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET
                 aur_url = excluded.aur_url,
                 auto_rebuild = MAX(auto_rebuild, excluded.auto_rebuild)",
        )
        .bind(name)
        .bind(aur_url)
        .bind(now)
        .bind(auto_rebuild as i64)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        let row = sqlx::query("SELECT id FROM packages WHERE name = ?")
            .bind(name)
            .fetch_one(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(row.get::<i64, _>(0))
    }

    /// Look up a package by its canonical name.
    pub async fn get_package_by_name(&self, name: &str) -> paur_core::Result<Option<Package>> {
        let row = sqlx::query(
            "SELECT id, name, aur_url, last_known_ref, added_at, enabled, auto_rebuild
             FROM packages WHERE name = ?",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(row_to_package))
    }

    /// List all packages, newest first.
    pub async fn list_packages(&self) -> paur_core::Result<Vec<Package>> {
        let rows = sqlx::query(
            "SELECT id, name, aur_url, last_known_ref, added_at, enabled, auto_rebuild
             FROM packages ORDER BY added_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows.into_iter().map(row_to_package).collect())
    }

    /// Delete a package by name. Returns the number of rows deleted (0 or 1).
    pub async fn delete_package(&self, name: &str) -> paur_core::Result<u64> {
        let res = sqlx::query("DELETE FROM packages WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(res.rows_affected())
    }

    /// Update the last-known AUR git ref for a package.
    pub async fn set_last_ref(&self, id: i64, git_ref: &str) -> paur_core::Result<()> {
        sqlx::query("UPDATE packages SET last_known_ref = ? WHERE id = ?")
            .bind(git_ref)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Toggle `enabled` flag.
    pub async fn set_package_enabled(&self, name: &str, enabled: bool) -> paur_core::Result<()> {
        sqlx::query("UPDATE packages SET enabled = ? WHERE name = ?")
            .bind(enabled as i64)
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    // -------- builds --------

    /// Enqueue a new build for a package. The build is created in
    /// `queued` state with the given trigger.
    pub async fn enqueue_build(
        &self,
        package_id: i64,
        trigger: BuildTrigger,
    ) -> paur_core::Result<i64> {
        let now = Self::now();
        let row = sqlx::query(
            "INSERT INTO builds (package_id, status, queued_at, trigger)
             VALUES (?, 'queued', ?, ?)
             RETURNING id",
        )
        .bind(package_id)
        .bind(now)
        .bind(trigger.as_str())
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.get::<i64, _>(0))
    }

    /// Claim the next queued build atomically and mark it running.
    /// Returns `None` if the queue is empty.
    pub async fn claim_next_queued(&self, worker_id: &str) -> paur_core::Result<Option<Build>> {
        let mut tx = self.pool.begin().await.map_err(db_err)?;
        let now = Self::now();
        // Pick the oldest queued build.
        let row = sqlx::query(
            "SELECT id, package_id, status, queued_at, started_at, finished_at, exit_code,
                    pkg_file, pkg_version, worker_id, trigger
             FROM builds WHERE status = 'queued'
             ORDER BY queued_at ASC, id ASC LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_err)?;
        let Some(row) = row else { return Ok(None) };
        let id: i64 = row.get(0);
        sqlx::query(
            "UPDATE builds SET status = 'running', started_at = ?, worker_id = ?
             WHERE id = ? AND status = 'queued'",
        )
        .bind(now)
        .bind(worker_id)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        // Re-read the now-running row.
        let row = sqlx::query(
            "SELECT id, package_id, status, queued_at, started_at, finished_at, exit_code,
                    pkg_file, pkg_version, worker_id, trigger
             FROM builds WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_err)?;
        tx.commit().await.map_err(db_err)?;
        Ok(Some(row_to_build(&row)))
    }

    /// Mark a build as finished with a final status.
    pub async fn finish_build(
        &self,
        id: i64,
        status: BuildStatus,
        exit_code: Option<i32>,
    ) -> paur_core::Result<()> {
        let now = Self::now();
        sqlx::query(
            "UPDATE builds SET status = ?, finished_at = ?, exit_code = ?
             WHERE id = ?",
        )
        .bind(status.as_str())
        .bind(now)
        .bind(exit_code)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Record the produced `.pkg.tar.zst` path and version for a build.
    pub async fn record_pkg(&self, id: i64, pkg_file: &str, pkg_version: &str) -> paur_core::Result<()> {
        sqlx::query("UPDATE builds SET pkg_file = ?, pkg_version = ? WHERE id = ?")
            .bind(pkg_file)
            .bind(pkg_version)
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(())
    }

    /// Fetch a build by id.
    pub async fn get_build(&self, id: i64) -> paur_core::Result<Option<Build>> {
        let row = sqlx::query(
            "SELECT id, package_id, status, queued_at, started_at, finished_at, exit_code,
                    pkg_file, pkg_version, worker_id, trigger
             FROM builds WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|r| row_to_build(&r)))
    }

    /// Fetch the most recent build for a package, if any.
    pub async fn latest_build_for(&self, package_id: i64) -> paur_core::Result<Option<Build>> {
        let row = sqlx::query(
            "SELECT id, package_id, status, queued_at, started_at, finished_at, exit_code,
                    pkg_file, pkg_version, worker_id, trigger
             FROM builds WHERE package_id = ?
             ORDER BY queued_at DESC, id DESC LIMIT 1",
        )
        .bind(package_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(row.map(|r| row_to_build(&r)))
    }

    /// List recent builds, optionally filtered.
    pub async fn list_builds(
        &self,
        pkg: Option<&str>,
        status: Option<BuildStatus>,
        limit: i64,
    ) -> paur_core::Result<Vec<Build>> {
        // Build a query dynamically. For simplicity, branch on filters.
        let mut sql = String::from(
            "SELECT b.id, b.package_id, b.status, b.queued_at, b.started_at, b.finished_at,
                    b.exit_code, b.pkg_file, b.pkg_version, b.worker_id, b.trigger
             FROM builds b",
        );
        let mut conds: Vec<String> = Vec::new();
        if pkg.is_some() {
            conds.push("b.package_id = (SELECT id FROM packages WHERE name = ?)".to_string());
        }
        if status.is_some() {
            conds.push("b.status = ?".to_string());
        }
        if !conds.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&conds.join(" AND "));
        }
        sql.push_str(" ORDER BY b.queued_at DESC, b.id DESC LIMIT ?");
        let mut q = sqlx::query(&sql);
        if let Some(p) = pkg {
            q = q.bind(p);
        }
        if let Some(s) = status {
            q = q.bind(s.as_str());
        }
        q = q.bind(limit);
        let rows = q.fetch_all(&self.pool).await.map_err(db_err)?;
        Ok(rows.into_iter().map(|r| row_to_build(&r)).collect())
    }

    /// On daemon startup, mark any builds left in `running` as `failed`
    /// (the worker that owned them is gone).
    pub async fn reap_stale_running(&self) -> paur_core::Result<u64> {
        let now = Self::now();
        let res = sqlx::query(
            "UPDATE builds SET status = 'failed', finished_at = ?, exit_code = -1
             WHERE status = 'running'",
        )
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(res.rows_affected())
    }

    // -------- logs --------

    /// Append a single log line for a build. The `seq` is allocated
    /// automatically as `(current max + 1)` for the build.
    pub async fn append_log(
        &self,
        build_id: i64,
        stream: Stream,
        line: &str,
    ) -> paur_core::Result<()> {
        let now = Self::now();
        let next_seq: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(seq), 0) + 1 FROM build_logs WHERE build_id = ?",
        )
        .bind(build_id)
        .fetch_one(&self.pool)
        .await
        .map_err(db_err)?;
        sqlx::query(
            "INSERT INTO build_logs (build_id, seq, stream, line, ts)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(build_id)
        .bind(next_seq)
        .bind(stream.as_str())
        .bind(line)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    /// Return all log lines for a build, ordered by seq.
    pub async fn read_logs(&self, build_id: i64) -> paur_core::Result<Vec<(Stream, String)>> {
        let rows = sqlx::query(
            "SELECT stream, line FROM build_logs WHERE build_id = ? ORDER BY seq ASC",
        )
        .bind(build_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let s: String = r.get(0);
                let line: String = r.get(1);
                let stream = s.parse::<Stream>().unwrap_or(Stream::Stdout);
                (stream, line)
            })
            .collect())
    }

    // -------- settings --------

    pub async fn get_setting(&self, key: &str) -> paur_core::Result<Option<String>> {
        let row = sqlx::query("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(row.map(|r| r.get::<String, _>(0)))
    }

    pub async fn set_setting(&self, key: &str, value: &str) -> paur_core::Result<()> {
        sqlx::query(
            "INSERT INTO settings(key, value) VALUES(?, ?)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(db_err)?;
        Ok(())
    }

    pub async fn all_settings(&self) -> paur_core::Result<Vec<Setting>> {
        let rows = sqlx::query("SELECT key, value FROM settings ORDER BY key ASC")
            .fetch_all(&self.pool)
            .await
            .map_err(db_err)?;
        Ok(rows
            .into_iter()
            .map(|r| Setting {
                key: r.get(0),
                value: r.get(1),
            })
            .collect())
    }
}

// -------- row mapping helpers --------

fn row_to_package(r: sqlx::sqlite::SqliteRow) -> Package {
    Package {
        id: r.get(0),
        name: r.get(1),
        aur_url: r.get(2),
        last_known_ref: r.get(3),
        added_at: r.get(4),
        enabled: r.get::<i64, _>(5) != 0,
        auto_rebuild: r.get::<i64, _>(6) != 0,
    }
}

fn row_to_build(r: &sqlx::sqlite::SqliteRow) -> Build {
    let status_s: String = r.get(2);
    let trigger_s: String = r.get(10);
    Build {
        id: r.get(0),
        package_id: r.get(1),
        status: status_s.parse().unwrap_or(BuildStatus::Failed),
        queued_at: r.get(3),
        started_at: r.get(4),
        finished_at: r.get(5),
        exit_code: r.get(6),
        pkg_file: r.get(7),
        pkg_version: r.get(8),
        worker_id: r.get(9),
        trigger: trigger_s.parse().unwrap_or(BuildTrigger::Manual),
    }
}

fn db_err(e: sqlx::Error) -> paur_core::Error {
    paur_core::Error::Db(e.to_string())
}
