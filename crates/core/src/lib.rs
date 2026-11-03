//! Frillogram's Telegram layer: a TDLib session and the app-level types the UI
//! works with. Nothing in here knows about GPUI.

mod client;
mod model;
pub mod threads;

pub use client::{Config, Telegram};
pub use model::{AuthStep, ChatSummary, Event, Msg, SenderId};
