//! The user's own AI: an OpenAI-compatible endpoint (the client, where its key is kept, the chat
//! that uses it and the one-click actions), or an AI agent installed on this computer that
//! speaks the Agent Client Protocol (found, started and spoken to here). Meshgrove brings no AI
//! of its own; nothing here runs unless the user set up an endpoint or has an agent.

pub mod acp;
pub mod acp_client;
pub mod actions;
pub mod agents;
pub mod chat;
pub mod client;
pub mod connect;
pub mod process;
pub mod secret;
