use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no Kobo database found at {0}")]
    NotAKobo(PathBuf),
    #[error(
        "{0} is on a Kobo e-reader. Kollate never writes to your Kobo; choose a location on this computer instead."
    )]
    OnKobo(PathBuf),
}

pub type Result<T> = std::result::Result<T, Error>;
