use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse ELF: {0}")]
    Elf(String),
    #[error("DWARF parsing failed: {0}")]
    Dwarf(String),
}

pub type Result<T> = std::result::Result<T, Error>;
