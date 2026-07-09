//! Pretty-print helpers for the CLI.
//!
//! We hand-roll a small table renderer instead of pulling in a
//! dependency. Output goes to stdout; nothing fancy.

use chrono::TimeZone;
use std::io::Write;

use crate::client::{BuildDto, PackageDto, QueueDto};

/// Format a unix epoch (seconds) as a short, human-readable local time
/// string. Returns `"-"` for `None` and the literal `"<epoch>"` for
/// values that overflow a reasonable timestamp range.
pub fn fmt_ts(ts: Option<i64>) -> String {
    match ts {
        None => "-".to_string(),
        Some(s) if s <= 0 => s.to_string(),
        Some(s) => match chrono::Local.timestamp_opt(s, 0).single() {
            Some(t) => t.format("%Y-%m-%d %H:%M:%S").to_string(),
            None => s.to_string(),
        },
    }
}

/// Print a list of packages as a fixed-width table.
pub fn print_packages(pkgs: &[PackageDto]) {
    println!(
        "{:<5} {:<30} {:<8} {:<12} {:<12} {}",
        "ID", "NAME", "AUTO", "STATUS", "VERSION", "ADDED"
    );
    for p in pkgs {
        let (status, version) = match &p.latest_build {
            Some(b) => (b.status.clone(), b.pkg_version.clone().unwrap_or_else(|| "-".into())),
            None => ("-".to_string(), "-".to_string()),
        };
        let added = p.id; // package DTO doesn't carry added_at today
        let _ = added; // suppress unused warning
        println!(
            "{:<5} {:<30} {:<8} {:<12} {:<12} {}",
            p.id,
            p.name,
            if p.auto_rebuild { "yes" } else { "no" },
            status,
            version,
            "-",
        );
    }
}

/// Print a list of builds as a table.
pub fn print_builds(rows: &[BuildDto], pkg_lookup: Option<&[PackageDto]>) {
    println!(
        "{:<5} {:<24} {:<20} {:<10} {:<8} {}",
        "ID", "QUEUED", "STATUS", "EXIT", "TRIGGER", "PKG"
    );
    for b in rows {
        let pkg_name = pkg_lookup
            .and_then(|pkgs| pkgs.iter().find(|p| p.id == b.package_id))
            .map(|p| p.name.clone())
            .unwrap_or_else(|| format!("#{}", b.package_id));
        let version = b.pkg_version.clone().unwrap_or_else(|| "-".into());
        println!(
            "{:<5} {:<24} {:<20} {:<10} {:<8} {} {}",
            b.id,
            fmt_ts(Some(b.queued_at)),
            b.status,
            b.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "-".into()),
            b.trigger,
            pkg_name,
            version,
        );
    }
}

/// Print the current queue state.
pub fn print_queue(q: &QueueDto) {
    println!("queued: {}", q.queued.len());
    println!("running: {}", q.running.len());
    if !q.queued.is_empty() {
        println!();
        println!("== queued ==");
        print_builds(&q.queued, None);
    }
    if !q.running.is_empty() {
        println!();
        println!("== running ==");
        print_builds(&q.running, None);
    }
}

/// Print the contents of a log blob to a writer, prefixing with a
/// header line. The blob is the raw text returned by
/// `/api/v1/builds/:id/logs.txt`.
pub fn write_log<W: Write>(out: &mut W, header: &str, blob: &str) -> std::io::Result<()> {
    writeln!(out, "== {header} ==")?;
    out.write_all(blob.as_bytes())?;
    if !blob.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(())
}
