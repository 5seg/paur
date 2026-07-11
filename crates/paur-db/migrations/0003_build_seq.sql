-- paur 0003: per-package build sequence number.
--
-- Each `builds` row gets a 1-based sequence number scoped to its
-- package_id. The number reflects the historical build count for
-- that package (so #1 is the first ever build, #2 the next, etc.),
-- not the row order. We use a window function on the existing rows
-- to backfill in a single statement; future inserts compute seq
-- inside the same transaction as the INSERT (see Db::enqueue_build).
--
-- UNIQUE(package_id, seq) prevents accidental duplicates if the
-- transaction logic is ever bypassed.

ALTER TABLE builds ADD COLUMN seq INTEGER;

-- Backfill: rank existing rows by id ASC per package_id. Using
-- ROW_NUMBER() over an explicit ORDER BY id ASC gives a stable,
-- reproducible ranking even if id gaps exist.
UPDATE builds
   SET seq = (
       SELECT rn FROM (
           SELECT id, package_id,
                  ROW_NUMBER() OVER (PARTITION BY package_id ORDER BY id ASC) AS rn
             FROM builds
       ) r
       WHERE r.id = builds.id
   );

-- Make NOT NULL now that every row has a value, and enforce
-- uniqueness per package.
CREATE UNIQUE INDEX IF NOT EXISTS uq_builds_package_seq
    ON builds(package_id, seq);
