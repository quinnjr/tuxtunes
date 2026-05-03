//! Linux desktop integrations: system tray, desktop notifications, MPRIS.
//!
//! Each submodule is gated behind the `start` entrypoint that lib.rs
//! calls during setup so failures don't block app launch.

pub mod tray;
