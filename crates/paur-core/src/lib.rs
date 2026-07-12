//! paur - Personal AUR pre-build service
//!
//! Shared types, configuration, error handling, and path resolution used
//! across all paur crates.

#![warn(missing_docs)]
#![warn(rust_2024_compatibility)]

pub mod auth;
pub mod build_flags;
pub mod config;
pub mod error;
pub mod logging;
pub mod name;
pub mod paths;

pub use build_flags::{PackageBuildFlags, PackageVariants, Variant};
pub use config::{Config, ContainerRuntime, Listen, S3Config};
pub use error::{Error, Result};
pub use name::PkgName;
pub use paths::Paths;
