//! `skill-curator`: download, adapt, and maintain third-party agent Skills in a
//! user-owned Collection.
//!
//! The binary is a thin wrapper around [`cli::main`]; all behaviour lives in
//! this library so it can be exercised by unit and integration tests.

pub mod adaptation;
pub mod cli;
pub mod collection;
pub mod diff;
mod error;
pub mod hash;
pub mod materialize;
pub mod safety;
pub mod source;

pub use error::Error;
