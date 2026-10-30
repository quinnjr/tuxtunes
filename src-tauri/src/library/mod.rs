//! File-level library operations (probe, ingest, organize).

pub mod artwork;
pub mod atomic;
pub mod genres;
pub mod ingest;
pub mod pacer;
pub mod releases;
pub mod rescan;
#[cfg(test)]
pub(crate) mod test_support;
