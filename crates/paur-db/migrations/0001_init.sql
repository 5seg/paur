-- paur initial schema
-- Tables: packages, builds, build_logs, settings
-- The build status is constrained to a fixed enum via CHECK.

CREATE TABLE IF NOT EXISTS packages (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL UNIQUE,
    aur_url         TEXT NOT NULL,
    last_known_ref  TEXT,
    added_at        INTEGER NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 1,
    auto_rebuild    INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX IF NOT EXISTS idx_packages_enabled ON packages(enabled);

CREATE TABLE IF NOT EXISTS builds (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    package_id   INTEGER NOT NULL REFERENCES packages(id) ON DELETE CASCADE,
    status       TEXT NOT NULL CHECK(status IN ('queued','running','success','failed','cancelled')),
    queued_at    INTEGER NOT NULL,
    started_at   INTEGER,
    finished_at  INTEGER,
    exit_code    INTEGER,
    pkg_file     TEXT,
    pkg_version  TEXT,
    worker_id    TEXT,
    trigger      TEXT NOT NULL DEFAULT 'manual'
        CHECK(trigger IN ('manual','poll','rebuild','dep'))
);

CREATE INDEX IF NOT EXISTS idx_builds_package_queued ON builds(package_id, queued_at DESC);
CREATE INDEX IF NOT EXISTS idx_builds_status ON builds(status);

CREATE TABLE IF NOT EXISTS build_logs (
    build_id  INTEGER NOT NULL REFERENCES builds(id) ON DELETE CASCADE,
    seq       INTEGER NOT NULL,
    stream    TEXT NOT NULL CHECK(stream IN ('stdout','stderr')),
    line      TEXT NOT NULL,
    ts        INTEGER NOT NULL,
    PRIMARY KEY (build_id, seq)
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- Seed default settings.
INSERT OR IGNORE INTO settings(key, value) VALUES ('repo_name', 'paur');
INSERT OR IGNORE INTO settings(key, value) VALUES ('arch', 'x86_64');
INSERT OR IGNORE INTO settings(key, value) VALUES ('schema_version', '1');
