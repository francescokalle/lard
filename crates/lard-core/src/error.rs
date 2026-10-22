use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("cannot serialize config {path}: {source}")]
    Serialize {
        path: PathBuf,
        #[source]
        source: toml::ser::Error,
    },
    #[error("invalid config: {0}")]
    Invalid(String),
}
