//! The user's own AI endpoint: the client, where its key is kept, and the chat that uses it.
//! Meshgrove brings no AI of its own; nothing here runs unless the user set up an endpoint.

pub mod chat;
pub mod client;
pub mod secret;
