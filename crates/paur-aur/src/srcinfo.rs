//! Minimal `.SRCINFO` parser. The format is documented at
//! <https://wiki.archlinux.org/title/.SRCINFO>.
//!
//! The parser is deliberately permissive: it ignores unknown keys
//! (forward-compat with new makepkg versions) and treats a single `=`
//! value as a one-element array. Multi-value entries use shell-style
//! parenthesised arrays.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Parsed `.SRCINFO` content. The shape mirrors what pacman sees for
/// a binary package: a top-level package plus a set of overrides
/// (`pkgbase_*`, then `pkgname = foo` blocks).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SrcInfo {
    /// Top-level (`pkgbase`) attributes. They are shared by all
    /// split packages unless overridden inside a `pkgname = ...` block.
    pub base: PkgInfo,
    /// Per-package overrides keyed by pkgname. Empty for a single
    /// non-split package.
    pub packages: BTreeMap<String, PkgInfo>,
}

/// Per-package (or per-pkgbase) attributes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PkgInfo {
    pub pkgname: Vec<String>,
    pub pkgver: Option<String>,
    pub pkgrel: Option<String>,
    pub epoch: Option<String>,
    pub pkgdesc: Option<String>,
    pub arch: Vec<String>,
    pub url: Option<String>,
    pub license: Vec<String>,
    pub depends: Vec<String>,
    pub makedepends: Vec<String>,
    pub checkdepends: Vec<String>,
    pub optdepends: Vec<String>,
    pub source: Vec<String>,
    pub sha256sums: Vec<String>,
    pub sha512sums: Vec<String>,
    pub b2sums: Vec<String>,
    pub provides: Vec<String>,
    pub conflicts: Vec<String>,
    pub replaces: Vec<String>,
}

impl SrcInfo {
    /// Convenience: "1.2.3-2" (combining `pkgver` and `pkgrel`, with
    /// an optional `epoch` prefix). Returns `None` if either is missing.
    pub fn full_version(&self) -> Option<String> {
        full_version(&self.base)
    }

    /// Full version of a specific package, including any overrides.
    pub fn full_version_of(&self, pkg: &str) -> Option<String> {
        let info = self.packages.get(pkg).unwrap_or(&self.base);
        full_version(info)
    }
}

fn full_version(info: &PkgInfo) -> Option<String> {
    let ver = info.pkgver.as_deref()?;
    let rel = info.pkgrel.as_deref()?;
    let epoch = info
        .epoch
        .as_deref()
        .filter(|e| !e.is_empty())
        .map(|e| format!("{e}:"))
        .unwrap_or_default();
    Some(format!("{epoch}{ver}-{rel}"))
}

impl PkgInfo {
    /// Merge `other` into `self` for keys `other` has set. Lets the
    /// caller layer per-package overrides on top of the pkgbase.
    pub fn merge_from(&mut self, other: &PkgInfo) {
        if !other.pkgname.is_empty() {
            self.pkgname = other.pkgname.clone();
        }
        if other.pkgver.is_some() {
            self.pkgver = other.pkgver.clone();
        }
        if other.pkgrel.is_some() {
            self.pkgrel = other.pkgrel.clone();
        }
        if other.epoch.is_some() {
            self.epoch = other.epoch.clone();
        }
        if other.pkgdesc.is_some() {
            self.pkgdesc = other.pkgdesc.clone();
        }
        if !other.arch.is_empty() {
            self.arch = other.arch.clone();
        }
        if other.url.is_some() {
            self.url = other.url.clone();
        }
        if !other.license.is_empty() {
            self.license = other.license.clone();
        }
        if !other.depends.is_empty() {
            self.depends = other.depends.clone();
        }
        if !other.makedepends.is_empty() {
            self.makedepends = other.makedepends.clone();
        }
        if !other.checkdepends.is_empty() {
            self.checkdepends = other.checkdepends.clone();
        }
        if !other.optdepends.is_empty() {
            self.optdepends = other.optdepends.clone();
        }
        if !other.source.is_empty() {
            self.source = other.source.clone();
        }
        if !other.sha256sums.is_empty() {
            self.sha256sums = other.sha256sums.clone();
        }
        if !other.sha512sums.is_empty() {
            self.sha512sums = other.sha512sums.clone();
        }
        if !other.b2sums.is_empty() {
            self.b2sums = other.b2sums.clone();
        }
        if !other.provides.is_empty() {
            self.provides = other.provides.clone();
        }
        if !other.conflicts.is_empty() {
            self.conflicts = other.conflicts.clone();
        }
        if !other.replaces.is_empty() {
            self.replaces = other.replaces.clone();
        }
    }
}

/// Parse a `.SRCINFO` string.
pub fn parse(input: &str) -> Result<SrcInfo, ParseError> {
    let mut current_pkg: Option<String> = None;
    let mut base = PkgInfo::default();
    let mut packages: BTreeMap<String, PkgInfo> = BTreeMap::new();

    for (lineno, raw) in input.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // Array form: `key = ( a b c )`
        if let Some(rest) = line.strip_suffix(')') {
            // Find the matching "= (" — start of the array.
            if let Some(eq_idx) = rest.find("= (") {
                let key = rest[..eq_idx].trim();
                let inside = &rest[eq_idx + 3..rest.len()]; // up to ')'
                let values: Vec<String> = inside
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect();
                apply_key(&mut base, &mut packages, &mut current_pkg, key, &values);
                continue;
            }
        }
        // Scalar form: `key = value`
        if let Some((k, v)) = line.split_once('=') {
            let key = k.trim();
            let value = v.trim().trim_matches(|c| c == '"' || c == '\'');
            apply_key(&mut base, &mut packages, &mut current_pkg, key, &[value.to_string()]);
            continue;
        }
        // If we got here, the line is not a key=value and not an array.
        return Err(ParseError {
            line: lineno + 1,
            content: raw.to_string(),
        });
    }

    // Propagate pkgbase fields onto per-package entries so callers
    // don't have to remember to read from `base` for unset keys.
    for info in packages.values_mut() {
        let mut merged = base.clone();
        merged.merge_from(info);
        *info = merged;
    }

    Ok(SrcInfo { base, packages })
}

#[derive(Debug, thiserror::Error)]
#[error("parse error on line {line}: {content:?}")]
pub struct ParseError {
    pub line: usize,
    pub content: String,
}

fn apply_key(
    base: &mut PkgInfo,
    packages: &mut BTreeMap<String, PkgInfo>,
    current_pkg: &mut Option<String>,
    key: &str,
    values: &[String],
) {
    // `pkgname` inside an override block switches the target. The
    // override block is delimited by the *next* `pkgname = ...` line.
    if key == "pkgname" {
        if let Some(name) = values.first() {
            // Detect override: anything set in `base` already means we
            // are past the pkgbase header (or we are in a split pkg).
            if !base.pkgname.is_empty() && current_pkg.as_deref() != Some(name.as_str()) {
                *current_pkg = Some(name.clone());
                packages.entry(name.clone()).or_default();
            } else {
                base.pkgname = values.to_vec();
            }
        }
        return;
    }

    let target: &mut PkgInfo = match current_pkg {
        Some(name) => packages.entry(name.clone()).or_default(),
        None => base,
    };

    match key {
        "pkgver" => target.pkgver = values.first().cloned(),
        "pkgrel" => target.pkgrel = values.first().cloned(),
        "epoch" => target.epoch = values.first().cloned(),
        "pkgdesc" => target.pkgdesc = values.first().cloned(),
        "url" => target.url = values.first().cloned(),
        // For list-valued keys, each occurrence is appended (matches
        // makepkg semantics: a `source = a` line followed by
        // `source = b` adds both).
        "arch" => target.arch.extend(values.iter().cloned()),
        "license" => target.license.extend(values.iter().cloned()),
        "depends" => target.depends.extend(values.iter().cloned()),
        "makedepends" => target.makedepends.extend(values.iter().cloned()),
        "checkdepends" => target.checkdepends.extend(values.iter().cloned()),
        "optdepends" => target.optdepends.extend(values.iter().cloned()),
        "source" => target.source.extend(values.iter().cloned()),
        "sha256sums" => target.sha256sums.extend(values.iter().cloned()),
        "sha512sums" => target.sha512sums.extend(values.iter().cloned()),
        "b2sums" => target.b2sums.extend(values.iter().cloned()),
        "provides" => target.provides.extend(values.iter().cloned()),
        "conflicts" => target.conflicts.extend(values.iter().cloned()),
        "replaces" => target.replaces.extend(values.iter().cloned()),
        // pkgbase is informational; the keys it unlocks are already
        // shared via the per-pkgname merge step.
        "pkgbase" => {}
        // Unknown keys: ignore, forward-compat.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal() {
        let input = "\
pkgbase = foo
pkgver = 1.2.3
pkgrel = 1
pkgname = foo
arch = ( x86_64 )
";
        let s = parse(input).unwrap();
        assert_eq!(s.base.pkgver.as_deref(), Some("1.2.3"));
        assert_eq!(s.base.pkgrel.as_deref(), Some("1"));
        assert_eq!(s.full_version(), Some("1.2.3-1".into()));
        assert_eq!(s.base.arch, vec!["x86_64"]);
        // Per-pkgname view inherits the base fields.
        assert_eq!(s.full_version_of("foo"), Some("1.2.3-1".into()));
    }

    #[test]
    fn handles_epoch_and_arrays() {
        let input = "\
pkgbase = libfoo
pkgver = 2.0
pkgrel = 3
epoch = 1
pkgname = libfoo
pkgname = libfoo-dev
depends = ( glibc gcc-libs )
license = ( GPL MIT )
makedepends = git
source = https://example.com/foo.tar.gz
source = https://example.com/foo.patch
arch = x86_64
";
        let s = parse(input).unwrap();
        assert_eq!(s.full_version(), Some("1:2.0-3".into()));
        // depends is an array and lives on the *override* package
        // (libfoo-dev) since the second pkgname line is what starts
        // the override block. The base itself does not get them.
        assert_eq!(s.packages["libfoo-dev"].depends, vec!["glibc", "gcc-libs"]);
        // license was array-form too, but happens after pkgname switch.
        assert_eq!(s.packages["libfoo-dev"].license, vec!["GPL", "MIT"]);
        // makedepends and source are scalar; they should still land
        // on the override target.
        assert_eq!(s.packages["libfoo-dev"].makedepends, vec!["git"]);
        assert_eq!(s.packages["libfoo-dev"].source.len(), 2);
    }

    #[test]
    fn ignores_comments() {
        let input = "# this is a comment\npkgbase = foo\npkgver = 1.0\n";
        let s = parse(input).unwrap();
        assert_eq!(s.base.pkgver.as_deref(), Some("1.0"));
    }

    #[test]
    fn unknown_keys_silently_ignored() {
        let input = "pkgbase = foo\npkgver = 1.0\ntotally_made_up = ( a b )\n";
        let s = parse(input).unwrap();
        assert_eq!(s.base.pkgver.as_deref(), Some("1.0"));
    }
}
