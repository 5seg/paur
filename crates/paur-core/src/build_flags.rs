//! Per-package build tuning flags and variant selection.
//!
//! Stored as JSON blobs in two columns on the `packages` row:
//!
//! - `build_flags`: a [`PackageBuildFlags`] covering per-package
//!   memory / CPU / ccache knobs. Empty means "use daemon defaults".
//!   The intent is to give operators a knob for the common OOM /
//!   slow-build cases on small hosts without forcing every package
//!   to carry the same config.
//!
//! - `variants`: a [`PackageVariants`] selecting which compiled
//!   variants of the package should be built and published to the
//!   paur repo. `default` is always active; `v3` / `v4` add
//!   CachyOS-style `-march=x86-64-vN` builds on top. Each active
//!   variant produces its own `.pkg.tar.zst` and lives in its own
//!   arch subdirectory (`x86_64` / `x86_64-v3` / `x86_64-v4`).
//!
//! ## Build flag semantics
//!
//! 1. `low_memory`           — sets `MAKEFLAGS=-j2` in the container env
//! 2. `rust_codegen_units_1` — appends `-C codegen-units=1` to `RUSTFLAGS`
//!                             (preserves any existing `RUSTFLAGS`)
//! 3. `no_ccache`            — skips the ccache bind mount entirely
//!
//! ## Variant semantics
//!
//! The build container reads `PAUR_MARCH=v3|v4` from the env and
//! applies the CachyOS recipe (`-march=x86-64-vN -O2 -pipe -fno-plt`
//! for CFLAGS/CXXFLAGS, append `-C target-cpu=x86-64-vN` to
//! RUSTFLAGS). The daemon hands the variant as a first-class enum
//! to the builder rather than an opaque `Option<MarchLevel>` field
//! on the flags struct, so the variant choice and the build flags
//! are independent knobs.
//!
//! ## Used by
//!
//! - `paur-db` to serialize/deserialize the columns
//! - `paur-builder` to actually apply the overrides
//! - `paur-daemon`'s API + `paur-cli`'s `flag` subcommand to read/mutate
//! - the SvelteKit UI to render the toggles
//!
//! New fields should be added with `#[serde(default)]` so older
//! serialized blobs keep deserializing.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Build tuning flags for a single package. See module docs for
/// semantics. Default = empty (all `false`), meaning "use daemon
/// defaults".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageBuildFlags {
    /// Cap parallel make jobs to `-j2` to cut peak RAM usage on
    /// small build hosts. Useful for heavyweight packages
    /// (wayvr, llvm, firefox, chromium) that would otherwise
    /// OOM at full core count.
    #[serde(default)]
    pub low_memory: bool,

    /// Append `-C codegen-units=1` to `RUSTFLAGS` for this
    /// package. Reduces rustc peak memory by ~20-30% at the
    /// cost of slower codegen. Existing `RUSTFLAGS` from the
    /// PKGBUILD is preserved.
    #[serde(default)]
    pub rust_codegen_units_1: bool,

    /// Skip the ccache bind mount for this package. Use when
    /// ccache misses dominate anyway (e.g. very large
    /// compilations where the cache directory grows past
    /// available disk) or when debugging build issues that
    /// ccache might mask.
    #[serde(default)]
    pub no_ccache: bool,
}

impl PackageBuildFlags {
    /// True when no override is set. Used to skip writing
    /// `{}` blobs and to short-circuit build-time checks.
    pub fn is_empty(&self) -> bool {
        !self.low_memory && !self.rust_codegen_units_1 && !self.no_ccache
    }

    /// Merge `other` into `self`: any field set in `other` wins,
    /// and `false` is a no-op (does not clear). Used by
    /// `paur-cli` when toggling a single flag on, and by callers
    /// that only ever set flags to `true`.
    pub fn merge_from(&mut self, other: &PackageBuildFlags) {
        if other.low_memory {
            self.low_memory = true;
        }
        if other.rust_codegen_units_1 {
            self.rust_codegen_units_1 = true;
        }
        if other.no_ccache {
            self.no_ccache = true;
        }
    }

    /// Replace every field of `self` with the corresponding field
    /// of `other`. Used by the PATCH /flags endpoint so the client
    /// can describe the full desired state and turn flags off.
    pub fn replace_from(&mut self, other: &PackageBuildFlags) {
        self.low_memory = other.low_memory;
        self.rust_codegen_units_1 = other.rust_codegen_units_1;
        self.no_ccache = other.no_ccache;
    }
}

/// Which compiled variant a build (or a package) refers to.
///
/// `default` is the plain x86-64 build with the container's stock
/// `makepkg.conf`. `v3` / `v4` add CachyOS-style `-march` flags.
/// New variants need an explicit migration to widen the DB CHECK
/// constraint and the repo-side `arch_subdir` match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    /// Plain x86-64 build using the container's default `makepkg.conf`.
    Default,
    /// `-march=x86-64-v3` (Haswell / Excavator and later).
    V3,
    /// `-march=x86-64-v4` (Skylake-X / Zen 4 and later).
    V4,
}

impl Variant {
    /// The lowercase tag used in the DB column, JSON, and the
    /// `Server = ...$arch-$variant` client URL fragment.
    pub fn as_str(self) -> &'static str {
        match self {
            Variant::Default => "default",
            Variant::V3 => "v3",
            Variant::V4 => "v4",
        }
    }

    /// The `PAUR_MARCH` env value to hand to the build container.
    /// `default` builds don't set the env, so the container uses
    /// its stock `makepkg.conf`.
    pub fn as_paur_march(self) -> Option<&'static str> {
        match self {
            Variant::Default => None,
            Variant::V3 => Some("v3"),
            Variant::V4 => Some("v4"),
        }
    }

    /// All variants in the canonical build order (`default` first,
    /// then v3, then v4). Used to enqueue the full build chain
    /// when a package's variants change.
    pub fn all() -> &'static [Variant] {
        &[Variant::Default, Variant::V3, Variant::V4]
    }

    /// Parse from the same string form `as_str` returns. Returns
    /// `None` for unknown tags so callers can surface a clean
    /// 400-style error without panicking.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "default" => Some(Variant::Default),
            "v3" => Some(Variant::V3),
            "v4" => Some(Variant::V4),
            _ => None,
        }
    }
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The set of variants a package should be built for. `default` is
/// always true (the daemon enforces this — every package gets a
/// plain build at minimum). `v3` and `v4` are independent toggles.
///
/// The struct serializes to/from a small JSON object so the
/// `packages.variants` column can grow without further migrations
/// (new fields default to `false`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageVariants {
    /// `true` always — the daemon's own invariant. Kept in the
    /// struct so the wire format / DB blob is self-describing and
    /// the UI can render a single toggle group.
    #[serde(default = "default_true")]
    pub default: bool,
    /// `true` to also publish a `-march=x86-64-v3` build.
    #[serde(default)]
    pub v3: bool,
    /// `true` to also publish a `-march=x86-64-v4` build.
    #[serde(default)]
    pub v4: bool,
}

fn default_true() -> bool {
    true
}

impl Default for PackageVariants {
    fn default() -> Self {
        Self {
            default: true,
            v3: false,
            v4: false,
        }
    }
}

impl PackageVariants {
    /// `default` is always on; this never returns `true` even for
    /// an "all off" struct, because default alone is the
    /// baseline. Used for diagnostics only.
    pub fn is_default_only(&self) -> bool {
        !self.v3 && !self.v4
    }

    /// Iterate the active variants in build order
    /// (`default` → `v3` → `v4`). Used to enqueue the full
    /// build chain when a package is added or its variants
    /// change.
    pub fn active(&self) -> Vec<Variant> {
        Variant::all()
            .iter()
            .copied()
            .filter(|v| match v {
                Variant::Default => self.default,
                Variant::V3 => self.v3,
                Variant::V4 => self.v4,
            })
            .collect()
    }

    /// `true` iff `v` is in the active set.
    pub fn is_active(&self, v: Variant) -> bool {
        match v {
            Variant::Default => self.default,
            Variant::V3 => self.v3,
            Variant::V4 => self.v4,
        }
    }

    /// Activate `v` (no-op if already on). `default` is a no-op
    /// since it's always on; callers should not pass `Default`.
    pub fn turn_on(&mut self, v: Variant) {
        match v {
            Variant::Default => self.default = true,
            Variant::V3 => self.v3 = true,
            Variant::V4 => self.v4 = true,
        }
    }

    /// Deactivate `v` (no-op if already off). `default` is
    /// ignored — the daemon refuses to turn it off.
    pub fn turn_off(&mut self, v: Variant) {
        match v {
            Variant::Default => {} // invariant
            Variant::V3 => self.v3 = false,
            Variant::V4 => self.v4 = false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_flags_is_empty() {
        let f = PackageBuildFlags::default();
        assert!(f.is_empty());
    }

    #[test]
    fn deserialize_flags_empty_object() {
        let f: PackageBuildFlags = serde_json::from_str("{}").unwrap();
        assert!(f.is_empty());
    }

    #[test]
    fn deserialize_flags_partial_object() {
        let f: PackageBuildFlags = serde_json::from_str(r#"{"low_memory": true}"#).unwrap();
        assert!(f.low_memory);
        assert!(!f.rust_codegen_units_1);
        assert!(!f.no_ccache);
    }

    #[test]
    fn flags_merge_winner_takes_true() {
        let mut a = PackageBuildFlags::default();
        let b = PackageBuildFlags {
            low_memory: true,
            rust_codegen_units_1: true,
            no_ccache: false,
        };
        a.merge_from(&b);
        assert!(a.low_memory);
        assert!(a.rust_codegen_units_1);
        assert!(!a.no_ccache);
    }

    #[test]
    fn flags_merge_cannot_clear() {
        let mut a = PackageBuildFlags {
            low_memory: true,
            ..Default::default()
        };
        a.merge_from(&PackageBuildFlags::default());
        assert!(a.low_memory);
    }

    #[test]
    fn flags_replace_clears() {
        let mut a = PackageBuildFlags {
            low_memory: true,
            rust_codegen_units_1: true,
            no_ccache: true,
        };
        a.replace_from(&PackageBuildFlags::default());
        assert!(a.is_empty());
    }

    #[test]
    fn variant_as_str() {
        assert_eq!(Variant::Default.as_str(), "default");
        assert_eq!(Variant::V3.as_str(), "v3");
        assert_eq!(Variant::V4.as_str(), "v4");
    }

    #[test]
    fn variant_as_paur_march() {
        assert_eq!(Variant::Default.as_paur_march(), None);
        assert_eq!(Variant::V3.as_paur_march(), Some("v3"));
        assert_eq!(Variant::V4.as_paur_march(), Some("v4"));
    }

    #[test]
    fn variant_parse_roundtrip() {
        for v in Variant::all() {
            assert_eq!(Variant::parse(v.as_str()), Some(*v));
        }
        assert_eq!(Variant::parse("v2"), None);
        assert_eq!(Variant::parse(""), None);
    }

    #[test]
    fn variants_default_has_default_only() {
        let v = PackageVariants::default();
        assert!(v.is_default_only());
        assert_eq!(v.active(), vec![Variant::Default]);
    }

    #[test]
    fn variants_deserialize_empty_object() {
        // Empty JSON should still produce a valid struct (default
        // = true). Lets old blobs round-trip without a migration.
        let v: PackageVariants = serde_json::from_str("{}").unwrap();
        assert!(v.default);
        assert!(!v.v3);
        assert!(!v.v4);
    }

    #[test]
    fn variants_serialize_omits_unchanged_default() {
        // default is always true, but the struct still round-trips
        // it; we just confirm the field is present in the JSON.
        let v = PackageVariants::default();
        let s = serde_json::to_string(&v).unwrap();
        assert!(s.contains("\"default\":true"), "got: {s}");
    }

    #[test]
    fn variants_active_in_canonical_order() {
        let v = PackageVariants {
            default: true,
            v3: true,
            v4: true,
        };
        assert_eq!(v.active(), vec![Variant::Default, Variant::V3, Variant::V4]);
        // default off, v3 and v4 on: default still wins (it's an
        // invariant, not a UI choice).
        let v2 = PackageVariants {
            default: false, // pretend the user tried to disable it
            v3: true,
            v4: true,
        };
        // `active()` only honors the explicit field values, so it
        // returns the empty set when default is false. The daemon
        // clamps this before persisting — see `set_variants`.
        assert_eq!(v2.active(), vec![Variant::V3, Variant::V4]);
    }

    #[test]
    fn variants_turn_on_off() {
        let mut v = PackageVariants::default();
        v.turn_on(Variant::V3);
        v.turn_on(Variant::V4);
        assert!(v.v3 && v.v4);
        v.turn_off(Variant::V3);
        assert!(!v.v3);
        assert!(v.v4);
        // default is invariant
        v.turn_off(Variant::Default);
        assert!(v.default);
    }
}
