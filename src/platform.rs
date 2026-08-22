//! Privilege detection — the single place allowed to touch OS FFI.
//!
//! Running elevated breaks file ownership for regular users (files written
//! by an admin/root process are then unwritable by the normal Spotify user),
//! which is why the whole binary refuses to run this way unless
//! `--bypass-admin` is passed. Mirrors the spicetify CLI's own guard.

/// True when the current process runs with administrator/root rights:
/// effective UID 0 on Linux and macOS, an elevated token on Windows.
pub fn is_privileged() -> bool {
    #[cfg(unix)]
    {
        // rustix exposes a safe wrapper over geteuid(2)
        rustix::process::geteuid().as_raw() == 0
    }

    #[cfg(windows)]
    {
        is_windows_elevated()
    }
}

#[cfg(windows)]
fn is_windows_elevated() -> bool {
    /// SAFETY: `IsUserAnAdmin` takes no arguments and only queries the
    /// calling process's token; it cannot dereference invalid memory.
    #[allow(unsafe_code)]
    unsafe {
        windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0
    }
}

/// The guard decision, separated from OS access for testability.
pub(crate) fn enforce(privileged: bool, bypass: bool) -> crate::errors::Result<()> {
    use crate::errors::Error;

    if privileged && !bypass {
        Err(Error::other(concat!(
            "refusing to run as administrator/root: elevated sessions leave ",
            "admin-owned files that break spicetify for the regular user later. ",
            "Re-run without sudo/as administrator, or pass --bypass-admin to override."
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_matrix() {
        assert!(
            enforce(true, false).is_err(),
            "privileged + no bypass denies"
        );
        assert!(enforce(true, true).is_ok(), "bypass overrides");
        assert!(enforce(false, false).is_ok());
        assert!(enforce(false, true).is_ok());

        let err = enforce(true, false).unwrap_err().to_string();
        assert!(err.contains("--bypass-admin"), "error must hint the flag");
    }

    #[test]
    fn detection_runs() {
        // value depends on the runner; we only require it to not panic and
        // to be a plain bool
        let _ = is_privileged();
    }
}
