//! Linux desktop integrations: system tray, desktop notifications, MPRIS.
//!
//! Each submodule is gated behind a setup entry point that lib.rs
//! calls during setup so failures don't block app launch.

pub mod mpris;
pub mod notify;
pub mod tray;

/// App-managed MPRIS handle. Wraps the optional Mpris since install
/// can fail (e.g. no session bus); consumers null-check before
/// mutating. Keeps both the shared state and the connection so signal
/// emission stays available alongside state mutation.
pub struct MprisHandle {
    pub mpris: Option<mpris::Mpris>,
}
