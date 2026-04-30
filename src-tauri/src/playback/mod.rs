//! Audio playback engine and its Tauri event bridge.

pub mod config;
pub mod device;
pub mod engine;
pub mod events;
pub mod stats;

pub use engine::{EngineCommand, EngineError, PlaybackEngine};
