//! Outbound device sync: pushing library tracks and playlists to MTP
//! devices, Android DAPs, and mounted storage.
//!
//! Distinct from [`crate::sync`], which runs the other way (an iTunes
//! ITL file into the library). Everything above [`transport`] sees only
//! the [`transport::DeviceTransport`] trait, so the platform-specific
//! backends stay a thin, swappable leaf.

pub mod layout;
pub mod manifest;
pub mod playlists;
pub mod transport;
