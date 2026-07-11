-- per-package build flags (memory/CPU tuning, OOM countermeasures)
-- JSON blob — empty {} means "use defaults from daemon config".
-- Flags:
--   low_memory             → MAKEFLAGS=-j2 (less peak RAM, slower)
--   rust_codegen_units_1   → RUSTFLAGS appends -C codegen-units=1
--   no_ccache              → skip ccache bind mount for this package
ALTER TABLE packages ADD COLUMN build_flags TEXT NOT NULL DEFAULT '{}';

UPDATE settings SET value = '4' WHERE key = 'schema_version';
