//! What the `yonder` programs share: the ledger (`store`), times and repeat
//! rules (`when`), running reminder checks (`check_exec`), masking secrets
//! (`redact`) and the config file (`config`).

pub mod check_exec;
pub mod config;
pub mod redact;
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
