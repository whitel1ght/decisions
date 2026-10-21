//! `decisions telegram`: the open decisions in the user's Telegram chat while they are away.

pub mod api;
pub mod bot;
#[cfg(test)]
pub mod fake;
pub mod render;
pub mod reply;
pub mod state;
