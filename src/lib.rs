//! `skill-curator`: download, adapt, and maintain third-party agent Skills in a
//! user-owned Collection.
//!
//! The binary is a thin wrapper around [`cli::main`]; all behaviour lives in
//! this library so it can be exercised by unit and integration tests.

pub mod cli;
mod error;

pub use error::Error;
