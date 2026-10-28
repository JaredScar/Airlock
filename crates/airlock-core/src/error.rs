use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid policy: {0}")]
    Policy(String),
    #[error("toml parse error: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("toml serialize error: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("unknown redaction id {0}")]
    UnknownRedaction(u64),
}

pub type Result<T> = std::result::Result<T, Error>;
