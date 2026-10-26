use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    #[error("failed to read {path}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse ELF")]
    Elf(#[from] object::read::Error),
    #[error("failed to parse DWARF")]
    Dwarf(#[from] gimli::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
