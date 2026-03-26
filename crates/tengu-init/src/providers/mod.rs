//! Cloud provider implementations

pub mod baremetal;
pub mod hetzner;

pub use baremetal::{Baremetal, TunnelConfig};
pub use hetzner::Hetzner;
