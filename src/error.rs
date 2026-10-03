use thiserror::Error;

/// Top-level application error.
#[derive(Debug, Error)]
pub enum Error {
    #[error("storage error: {0}")]
    Storage(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("config error: {0}")]
    Config(String),

    #[error("network error: {0}")]
    Network(#[from] crate::network::NetworkError),

    #[error("secret error: {0}")]
    Secret(String),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("image error: {0}")]
    Image(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_render_human_messages() {
        let e = Error::Config("bad toml".into());
        assert_eq!(e.to_string(), "config error: bad toml");
        let e = Error::Secret("missing".into());
        assert_eq!(e.to_string(), "secret error: missing");
        let e = Error::Image("corrupt".into());
        assert_eq!(e.to_string(), "image error: corrupt");
        let e = Error::InvalidArgument("x".into());
        assert_eq!(e.to_string(), "invalid argument: x");
        let e = Error::NotFound("topic 3".into());
        assert_eq!(e.to_string(), "not found: topic 3");
    }

    #[test]
    fn sqlite_errors_convert_into_storage() {
        let e: Error = rusqlite::Error::InvalidColumnName("c".into()).into();
        assert!(matches!(e, Error::Storage(_)));
        assert!(e.to_string().starts_with("storage error:"));
    }

    #[test]
    fn io_errors_convert_into_io_variant() {
        let e: Error = std::io::Error::other("disk").into();
        assert!(matches!(e, Error::Io(_)));
        assert!(e.to_string().contains("io error:"));
    }

    #[test]
    fn json_errors_convert_into_json_variant() {
        let e: Error = serde_json::from_str::<serde_json::Value>("{bad").unwrap_err().into();
        assert!(matches!(e, Error::Json(_)));
    }

    #[test]
    fn result_alias_wraps_std_result() {
        let ok: Result<i32> = Ok(1);
        assert_eq!(ok.unwrap(), 1);
    }
}
