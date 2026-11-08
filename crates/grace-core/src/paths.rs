//! Shared runtime paths.

use std::path::PathBuf;

/// Runtime control Unix socket. Lives in the user's temp directory:
/// it is the same for an `.app` started from Finder and for `grace-ctl` in a terminal,
/// and does not depend on where the repository is checked out.
pub fn control_socket() -> PathBuf {
    if let Some(p) = std::env::var_os("GRACE_SOCKET") {
        return PathBuf::from(p);
    }
    std::env::temp_dir().join("elder-grace").join("app.sock")
}
