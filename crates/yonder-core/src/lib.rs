//! What the `yonder` programs share: Yonder's home directory (`home`), bot
//! definitions (`bot`), the ledger (`store`, and `firing` for
//! what yonderd does to it), times and repeat rules (`when`), running
//! reminder checks (`check_exec`), the lead agent's terminal (`hc`),
//! yonderd's rounds (`daemon`) and its launchd plist (`service`), masking secrets (`redact`) and the config
//! file (`config`).

pub mod bot;
pub mod check_exec;
pub mod config;
pub mod daemon;
pub mod firing;
pub mod hc;
pub mod home;
pub mod redact;
pub mod service;
pub mod store;
pub mod when;

pub use store::{NewReminder, Store, StoreError};

/// A fresh, empty directory for one test.
#[cfg(test)]
fn test_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("yonder-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
