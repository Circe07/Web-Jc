//! orq-cache: content-addressed cache of verification results.
//!
//! The cache key is a BLAKE3 hash over everything that can influence the
//! outcome of a verification (see [`RedbCache`] for the exact list). Results
//! are stored as JSON in a [`redb`] database, with a simple LRU bound.

mod inputs;
mod store;

pub use store::RedbCache;

#[cfg(test)]
mod tests;
