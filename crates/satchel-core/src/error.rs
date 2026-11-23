use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not found")]
    NotFound,
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("clip too large: {size} bytes (limit {limit})")]
    TooLarge { size: u64, limit: u64 },
}

pub type Result<T> = std::result::Result<T, Error>;
