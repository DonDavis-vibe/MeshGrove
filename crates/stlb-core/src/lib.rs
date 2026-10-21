//! Core of the STL browser: everything that does not need a window or a GPU.

pub mod cache;
pub mod config;
pub mod db;
pub mod dedupe;
pub mod formats;
pub mod listing;
pub mod loaders;
pub mod mesh;
pub mod query;
pub mod scan;

pub use formats::Kind;
pub use mesh::{Aabb, Mesh};
