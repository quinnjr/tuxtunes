//! Linux desktop integrations: system tray, desktop notifications, MPRIS.
//!
//! Each submodule is gated behind a setup entry point that lib.rs
//! calls during setup so failures don't block app launch.

pub mod notify;
pub mod tray;
