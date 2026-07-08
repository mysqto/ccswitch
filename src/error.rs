//! Error types shared across the crate.

use thiserror::Error;

/// Errors produced by `ccswitch` operations.
#[derive(Debug, Error)]
pub enum Error {
    /// A profile name was not found in the store.
    #[error("profile '{0}' not found")]
    ProfileNotFound(String),

    /// A profile name collides with a reserved subcommand word.
    #[error("'{0}' is a reserved word, pick another profile name")]
    ReservedName(String),

    /// A required input was missing or empty.
    #[error("{0}")]
    Invalid(String),

    /// A wrapped I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// A wrapped JSON (de)serialization error.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Convenience result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_domain_variants() {
        assert_eq!(
            Error::ProfileNotFound("dev".to_string()).to_string(),
            "profile 'dev' not found"
        );
        assert_eq!(
            Error::ReservedName("list".to_string()).to_string(),
            "'list' is a reserved word, pick another profile name"
        );
        assert_eq!(
            Error::Invalid("bad input".to_string()).to_string(),
            "bad input"
        );
    }

    #[test]
    fn wraps_io_errors() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let err: Error = io.into();
        assert!(format!("{err:?}").starts_with("Io"));
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn wraps_json_errors() {
        let json_err = serde_json::from_str::<i32>("not json").unwrap_err();
        let err: Error = json_err.into();
        assert!(format!("{err:?}").starts_with("Json"));
        assert!(!err.to_string().is_empty());
    }
}
