//! Core of the STL browser: everything that does not need a window or a GPU.

pub mod dedupe;
pub mod formats;
pub mod loaders;
pub mod mesh;

pub use formats::Kind;
pub use mesh::{Aabb, Mesh};
