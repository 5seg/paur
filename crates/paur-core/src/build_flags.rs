//! Per-package build tuning flags.
//!
//! Stored in `packages.build_flags` as a JSON blob. Empty flags mean
//! "use daemon defaults". The intent is to give operators a knob for
//! the common OOM / slow-build cases on small hosts without forcing
//! every package to carry the same config.
//!
//! Each flag is a self-contained override; the daemon composes them
//! in a fixed order so behavior is predictable:
//!
//! 1. `low_memory`           — sets `MAKEFLAGS=-j2` in the container env
//! 2. `rust_codegen_units_1` — appends `-C codegen-units=1` to `RUSTFLAGS`
//!                             (preserves any existing `RUSTFLAGS`)
//! 3. `no_ccache`            — skips the ccache bind mount entirely
//! 4. `march`                — x86-64 microarchitecture level (v3 / v4).
//!                             Replaces CFLAGS with a CachyOS-style
//!                             `-march=x86-64-vN -O2 -pipe -fno-plt`,
//!                             sets CXXFLAGS to the same, and appends
//!                             `-C target-cpu=x86-64-vN` to RUSTFLAGS.
//!
//! The same struct is used by:
//! - `paur-db` to serialize/deserialize the column
//! - `paur-builder` to actually apply the overrides
//! - `paur-daemon`'s API + `paur-cli`'s `flag` subcommand to read/mutate
//! - the SvelteKit UI to render the toggles
//!
//! New flags should be added with `#[serde(default)]` so older
//! serialized blobs keep deserializing.

use serde::{Deserialize, Serialize};

/// x86-64 microarchitecture level. Determines which CPU features
/// the package's C / C++ / Rust code may assume at build time.
/// See <https://wiki.cachyos.org/cachyos_basic/why_cachyos/> for
/// the performance rationale.
///
/// Serialized as the lowercase string of the variant name
/// (`"v3"` / `"v4"`). Adding a new variant (e.g. `V2`) is the only
/// place to touch: the build.sh `apply_march` case statement must
/// learn the new value, and the SvelteKit UI must add a button.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarchLevel {
    /// x86-64-v3 — Haswell / Excavator and later. AVX2, BMI2, FMA.
    V3,
    /// x86-64-v4 — Skylake-X / Zen 4 and later. Adds AVX-512.
    V4,
}

impl MarchLevel {
    /// Lowercase string used both for the JSON value and the
    /// `PAUR_MARCH` env var that crosses into the build container.
    pub fn as_str(self) -> &'static str {
        match self {
            MarchLevel::V3 => "v3",
            MarchLevel::V4 => "v4",
        }
    }
}

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

    /// x86-64 microarchitecture level. `None` means "use the
    /// container's default `makepkg.conf`" (i.e. generic x86-64).
    /// Set to `Some(V3)` / `Some(V4)` to opt this package into
    /// CachyOS-style `-march=...` builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub march: Option<MarchLevel>,
}

impl PackageBuildFlags {
    /// True when no override is set. Used to skip writing
    /// `{}` blobs and to short-circuit build-time checks.
    pub fn is_empty(&self) -> bool {
        !self.low_memory
            && !self.rust_codegen_units_1
            && !self.no_ccache
            && self.march.is_none()
    }

    /// Merge `other` into `self`: any field set in `other` wins,
    /// and `false` / `None` is a no-op (does not clear). Used by
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
        if let Some(level) = other.march {
            self.march = Some(level);
        }
    }

    /// Replace every field of `self` with the corresponding field
    /// of `other`. Used by the PATCH /flags endpoint so the client
    /// can describe the full desired state and turn flags off.
    pub fn replace_from(&mut self, other: &PackageBuildFlags) {
        self.low_memory = other.low_memory;
        self.rust_codegen_units_1 = other.rust_codegen_units_1;
        self.no_ccache = other.no_ccache;
        self.march = other.march;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let f = PackageBuildFlags::default();
        assert!(f.is_empty());
        assert!(!f.low_memory);
        assert!(!f.rust_codegen_units_1);
        assert!(!f.no_ccache);
        assert!(f.march.is_none());
    }

    #[test]
    fn deserialize_empty_object() {
        let f: PackageBuildFlags = serde_json::from_str("{}").unwrap();
        assert!(f.is_empty());
    }

    #[test]
    fn deserialize_partial_object() {
        let f: PackageBuildFlags = serde_json::from_str(r#"{"low_memory": true}"#).unwrap();
        assert!(f.low_memory);
        assert!(!f.rust_codegen_units_1);
        assert!(!f.no_ccache);
        assert!(f.march.is_none());
    }

    #[test]
    fn deserialize_full_object() {
        let f: PackageBuildFlags = serde_json::from_str(
            r#"{"low_memory": true, "rust_codegen_units_1": true, "no_ccache": true}"#,
        )
        .unwrap();
        assert_eq!(
            f,
            PackageBuildFlags {
                low_memory: true,
                rust_codegen_units_1: true,
                no_ccache: true,
                march: None,
            }
        );
    }

    #[test]
    fn deserialize_march_lowercase() {
        let v3: PackageBuildFlags =
            serde_json::from_str(r#"{"march": "v3"}"#).unwrap();
        assert_eq!(v3.march, Some(MarchLevel::V3));
        let v4: PackageBuildFlags =
            serde_json::from_str(r#"{"march": "v4"}"#).unwrap();
        assert_eq!(v4.march, Some(MarchLevel::V4));
    }

    #[test]
    fn serialize_march_omits_none() {
        // skip_serializing_if keeps old blobs that only carry
        // bool flags from gaining a useless `"march": null`
        // entry on round-trip.
        let f = PackageBuildFlags {
            low_memory: true,
            ..Default::default()
        };
        let s = serde_json::to_string(&f).unwrap();
        assert!(!s.contains("march"), "got: {s}");
    }

    #[test]
    fn serialize_march_lowercase() {
        let f = PackageBuildFlags {
            march: Some(MarchLevel::V4),
            ..Default::default()
        };
        let s = serde_json::to_string(&f).unwrap();
        assert!(s.contains(r#""march":"v4""#), "got: {s}");
    }

    #[test]
    fn serialize_roundtrip() {
        let f = PackageBuildFlags {
            low_memory: true,
            rust_codegen_units_1: false,
            no_ccache: true,
            march: Some(MarchLevel::V3),
        };
        let s = serde_json::to_string(&f).unwrap();
        let back: PackageBuildFlags = serde_json::from_str(&s).unwrap();
        assert_eq!(f, back);
    }

    #[test]
    fn merge_winner_takes_true() {
        let mut a = PackageBuildFlags {
            low_memory: false,
            ..Default::default()
        };
        let b = PackageBuildFlags {
            low_memory: true,
            rust_codegen_units_1: true,
            no_ccache: false,
            march: Some(MarchLevel::V4),
        };
        a.merge_from(&b);
        assert!(a.low_memory);
        assert!(a.rust_codegen_units_1);
        assert!(!a.no_ccache);
        assert_eq!(a.march, Some(MarchLevel::V4));
    }

    #[test]
    fn merge_cannot_clear_flags() {
        // merge_from is one-way: setting a flag in `other` to false
        // does not clear it in `self`. The PATCH endpoint handles
        // explicit clears via a separate "set to false" path.
        let mut a = PackageBuildFlags {
            low_memory: true,
            march: Some(MarchLevel::V3),
            ..Default::default()
        };
        let b = PackageBuildFlags::default();
        a.merge_from(&b);
        assert!(a.low_memory, "merge_from must not clear existing flags");
        assert_eq!(a.march, Some(MarchLevel::V3));
    }

    #[test]
    fn replace_clears_flags() {
        // replace_from mirrors the full desired state, including
        // turning flags off. The PATCH /flags endpoint uses this so
        // a client can describe the complete state of the toggles
        // it just rendered and have the server reflect that — e.g.
        // sending {low_memory: false, ...} must turn low_memory off.
        let mut a = PackageBuildFlags {
            low_memory: true,
            rust_codegen_units_1: true,
            no_ccache: true,
            march: Some(MarchLevel::V3),
        };
        let b = PackageBuildFlags::default();
        a.replace_from(&b);
        assert!(a.is_empty());
    }

    #[test]
    fn replace_overrides_march() {
        // replace_from must swap march outright, not merge it.
        let mut a = PackageBuildFlags {
            march: Some(MarchLevel::V3),
            ..Default::default()
        };
        let b = PackageBuildFlags {
            march: Some(MarchLevel::V4),
            ..Default::default()
        };
        a.replace_from(&b);
        assert_eq!(a.march, Some(MarchLevel::V4));
    }

    #[test]
    fn march_level_as_str() {
        assert_eq!(MarchLevel::V3.as_str(), "v3");
        assert_eq!(MarchLevel::V4.as_str(), "v4");
    }
}
