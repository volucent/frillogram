//! Frillogram's Telegram layer: a TDLib session and the app-level types the UI
//! works with. Nothing in here knows about GPUI.

mod client;
pub mod config;
mod model;
pub mod threads;

pub use client::Telegram;
pub use config::{Config, ConfigError, Loaded, Settings};
pub use model::{
    Animation, AuthStep, ChatSummary, Event, Media, Msg, Photo, SendState, SenderId, Thumbnail, ThumbnailFormat, Upload,
};
