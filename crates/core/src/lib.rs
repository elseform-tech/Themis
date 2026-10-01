//! Themis agent core.
//!
//! Agent definitions, providers, tools, and thread state for the Themis
//! desktop coding-agent app. This is a stub: the public API will grow as
//! agent features land.

pub mod plugins;
pub mod providers;
pub mod runtime;
pub mod skills;
pub mod tools;

/// Returns the `themis-core` crate version.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_matches_package_version() {
        assert_eq!(version(), "0.1.0");
    }
}
