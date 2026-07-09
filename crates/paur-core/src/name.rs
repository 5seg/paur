//! Package name newtype with strict validation.
//!
//! AUR package names match the pacman package-name grammar:
//! lowercase letters, digits, and `._+-`, must start with a letter or digit.
//! This validator is intentionally stricter than what pacman accepts (no
//! version constraints, no `>=`, no `/` etc.) because we only ever handle
//! *base names* of AUR packages.

use std::fmt;
use std::str::FromStr;

use regex::Regex;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// Validated AUR package name. Always lowercase, matches `^[a-z0-9][a-z0-9._+-]*$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PkgName(String);

impl PkgName {
    /// Maximum allowed length for an AUR package name (matches pacman's limit).
    pub const MAX_LEN: usize = 64;

    /// Validate and construct a new [`PkgName`]. Returns an error with a
    /// human-readable reason if the name is invalid.
    pub fn new(s: &str) -> Result<Self> {
        let s = s.trim();
        if s.is_empty() {
            return Err(Error::InvalidName(s.to_string(), "name is empty".into()));
        }
        if s.len() > Self::MAX_LEN {
            return Err(Error::InvalidName(
                s.to_string(),
                format!("name exceeds {} chars", Self::MAX_LEN),
            ));
        }
        // Cheap fast-path rejection of obvious garbage.
        if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(Error::InvalidName(
                s.to_string(),
                "whitespace or control characters not allowed".into(),
            ));
        }
        // Full grammar check. Compile-once via OnceLock would be ideal; for
        // simplicity we compile on each call (validation is rare and on the
        // user-input path, not a hot loop).
        let re = Regex::new(r"^[a-z0-9][a-z0-9._+-]*$").expect("static regex is valid");
        if !re.is_match(s) {
            return Err(Error::InvalidName(
                s.to_string(),
                "name must match ^[a-z0-9][a-z0-9._+-]*$ (lowercase, no leading symbol)".into(),
            ));
        }
        Ok(PkgName(s.to_string()))
    }

    /// Borrow the underlying string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PkgName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for PkgName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for PkgName {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

impl Serialize for PkgName {
    fn serialize<S: Serializer>(&self, ser: S) -> std::result::Result<S::Ok, S::Error> {
        ser.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PkgName {
    fn deserialize<D: Deserializer<'de>>(de: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(de)?;
        PkgName::new(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        for n in ["paru-bin", "yay", "spotify", "a", "1", "foo.bar", "x86_64"] {
            assert!(PkgName::new(n).is_ok(), "expected {n:?} to be valid");
        }
    }

    #[test]
    fn invalid_names() {
        for n in ["", " ", "..bad", "-foo", "FOO", "foo bar", "foo/bar", "foo$bar"] {
            assert!(PkgName::new(n).is_err(), "expected {n:?} to be invalid");
        }
    }

    #[test]
    fn too_long_rejected() {
        let n = "a".repeat(PkgName::MAX_LEN + 1);
        assert!(PkgName::new(&n).is_err());
    }

    #[test]
    fn serde_roundtrip() {
        let n: PkgName = serde_json::from_str(r#""paru-bin""#).unwrap();
        assert_eq!(n.as_str(), "paru-bin");
        let s = serde_json::to_string(&n).unwrap();
        assert_eq!(s, r#""paru-bin""#);
    }
}
