//! The library as plugins may read it (`library` in the WIT world). Meshgrove's app and
//! meshgrove-mcp implement it within the user's AI scope; the host has already checked that the
//! user granted `library_read` before any of these is called.

use meshgrove_core::config::LibraryRead;
use meshgrove_plugin_api::MeshData;

pub trait LibraryAccess: Send + Sync {
    /// Matches for a search as typed in Meshgrove, as JSON (ids, names, sizes).
    fn search(&self, query: &str, limit: u32, scope: LibraryRead) -> Result<serde_json::Value, String>;
    /// Facts of one file by its id, as JSON.
    fn describe(&self, id: &str, scope: LibraryRead) -> Result<serde_json::Value, String>;
    /// The model's triangles, simplified to at most `max_triangles`.
    fn triangles(&self, id: &str, max_triangles: u32, scope: LibraryRead) -> Result<MeshData, String>;
}
