-- per-package build variants (default / v3 / v4).
--
-- Two changes:
--   packages.variants: JSON blob selecting which compiled variants
--       a package should produce. default is always true; v3/v4
--       are independent toggles. The empty object deserializes to
--       `{"default":true,"v3":false,"v4":false}` (see
--       PackageVariants::default), so old rows get a sane state
--       without explicit data backfill.
--   builds.variant: which variant this particular build is for.
--       CHECK constraint narrows the column to the three known
--       values; new variants need a follow-up migration.
ALTER TABLE packages ADD COLUMN variants TEXT NOT NULL DEFAULT '{}';
ALTER TABLE builds ADD COLUMN variant TEXT NOT NULL DEFAULT 'default'
    CHECK(variant IN ('default','v3','v4'));

CREATE INDEX IF NOT EXISTS idx_builds_variant ON builds(variant);

UPDATE settings SET value = '5' WHERE key = 'schema_version';
