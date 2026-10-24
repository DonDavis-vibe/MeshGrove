//! The user's own AI endpoint: the client, where its key is kept, the chat that uses it and the
//! one-click actions. Meshgrove brings no AI of its own; nothing here runs unless the user set
//! up an endpoint.

pub mod actions;
pub mod chat;
pub mod client;
pub mod secret;
