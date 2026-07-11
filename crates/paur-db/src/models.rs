//! Domain types and their SQLite encoding.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Build lifecycle state. Maps to a `CHECK` constraint in the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildStatus {
    Queued,
    Running,
    Success,
    Failed,
    Cancelled,
}

impl BuildStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildStatus::Queued => "queued",
            BuildStatus::Running => "running",
            BuildStatus::Success => "success",
            BuildStatus::Failed => "failed",
            BuildStatus::Cancelled => "cancelled",
        }
    }
}

impl FromStr for BuildStatus {
    type Err = paur_core::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "queued" => BuildStatus::Queued,
            "running" => BuildStatus::Running,
            "success" => BuildStatus::Success,
            "failed" => BuildStatus::Failed,
            "cancelled" => BuildStatus::Cancelled,
            other => return Err(paur_core::Error::Db(format!("unknown status: {other}"))),
        })
    }
}

/// What initiated a build. The schema constrains this to a fixed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BuildTrigger {
    Manual,
    Poll,
    Rebuild,
    Dep,
}

impl BuildTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            BuildTrigger::Manual => "manual",
            BuildTrigger::Poll => "poll",
            BuildTrigger::Rebuild => "rebuild",
            BuildTrigger::Dep => "dep",
        }
    }
}

impl FromStr for BuildTrigger {
    type Err = paur_core::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "manual" => BuildTrigger::Manual,
            "poll" => BuildTrigger::Poll,
            "rebuild" => BuildTrigger::Rebuild,
            "dep" => BuildTrigger::Dep,
            other => return Err(paur_core::Error::Db(format!("unknown trigger: {other}"))),
        })
    }
}

/// Log stream a line came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Stream {
    Stdout,
    Stderr,
}

impl Stream {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stream::Stdout => "stdout",
            Stream::Stderr => "stderr",
        }
    }
}

impl FromStr for Stream {
    type Err = paur_core::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "stdout" => Stream::Stdout,
            "stderr" => Stream::Stderr,
            other => return Err(paur_core::Error::Db(format!("unknown stream: {other}"))),
        })
    }
}

/// A row of the `packages` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Package {
    pub id: i64,
    pub name: String,
    pub aur_url: String,
    pub last_known_ref: Option<String>,
    pub added_at: i64,
    pub enabled: bool,
    pub auto_rebuild: bool,
    /// Per-package build tuning (memory/CPU). JSON blob from
    /// `PackageBuildFlags`. Defaults to all-false when the column
    /// is empty (older rows from before migration 0004).
    #[serde(default)]
    pub build_flags: paur_core::PackageBuildFlags,
}

/// A row of the `builds` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Build {
    pub id: i64,
    pub package_id: i64,
    /// 1-based per-package sequence number: #1 is the first build for
    /// this package, #2 the next, etc. Stable for the lifetime of a
    /// row; the global `id` is still the canonical primary key.
    pub seq: i64,
    pub status: BuildStatus,
    pub queued_at: i64,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub exit_code: Option<i64>,
    pub pkg_file: Option<String>,
    pub pkg_version: Option<String>,
    pub worker_id: Option<String>,
    pub trigger: BuildTrigger,
}

/// A row of the `settings` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Setting {
    pub key: String,
    pub value: String,
}
