//! File-level library operations (probe, ingest, organize).

pub mod artwork;
pub mod genres;
pub mod ingest;
pub mod rescan;
#[cfg(test)]
pub(crate) mod test_support;
