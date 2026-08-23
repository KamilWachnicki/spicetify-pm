//! `self-update`: compare the running version against the latest GitHub
//! release and, when outdated, re-run the platform install script pinned to
//! that release over this binary in place.

use crate::errors::{Error, Result};
use crate::http::HttpClient;
use crate::market::constants::{install_script_raw_url, latest_release_api_url};
use crate::ui;
use serde::Deserialize;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Deserialize)]
struct LatestRelease {
    tag_name: String,
}

/// Outcome of comparing the running version with the latest tag.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    UpToDate,
    UpdateAvailable,
}

pub async fn run(http: &HttpClient, check: bool, yes: bool) -> Result<()> {
    if !cfg!(any(target_os = "linux", target_os = "macos", target_os = "windows")) {
        return Err(Error::other(
            "self-update supports linux, macOS and Windows only; reinstall manually",
        ));
    }

    let current = env!("CARGO_PKG_VERSION");
    let spinner = ui::spinner(check, "fetching the latest release");
    let release: LatestRelease = http.get_json(&latest_release_api_url()).await?;
    let latest = release.tag_name;
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    match verdict_for(current, &latest) {
        Verdict::UpToDate => {
            ui::success(format!("spice-pm {current} is up to date (latest release {latest})"));
            return Ok(());
        }
        // CI-friendly: an existing update is signalled through the exit code
        Verdict::UpdateAvailable if check => {
            ui::info(format!("update available: {current} -> {latest}"));
            std::process::exit(1);
        }
        Verdict::UpdateAvailable => {}
    }

    if !yes {
        let confirmed =
            dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
                .with_prompt(format!("update spice-pm {current} -> {latest}?"))
                .default(false)
                .interact()
                .unwrap_or(false);
        if !confirmed {
            ui::info("aborted - staying on the current version");
            return Ok(());
        }
    }

    update_in_place(http, &latest).await
}

/// Re-install over the running binary using the repo's install script at the
/// target tag. The running executable is renamed aside first (allowed even
/// while running on every supported OS) so the script never trips over file
/// locks; it is restored if anything goes wrong.
async fn update_in_place(http: &HttpClient, tag: &str) -> Result<()> {
    let exe = std::env::current_exe()
        .map_err(|e| Error::other(format!("cannot locate the running binary: {e}")))?;
    let dir = exe
        .parent()
        .ok_or_else(|| Error::other("running binary has no parent directory"))?
        .to_path_buf();

    let script_bytes = download_script(http, tag).await?;
    let tmp = write_temp_script(&script_bytes, tag)?;

    let Some(name) = exe.file_name() else {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::other("running binary has no file name"));
    };
    let mut old_name = name.to_os_string();
    old_name.push(".old");
    let staged = exe.with_file_name(old_name);

    if let Err(e) = std::fs::rename(&exe, &staged) {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::other(format!(
            "cannot stage `{}` for replacement ({e}); re-run with the permissions that installed it",
            exe.display()
        )));
    }

    match run_installer(&tmp, tag, &dir) {
        Ok(()) => {
            let _ = std::fs::remove_file(&staged);
            let _ = std::fs::remove_file(&tmp);
            ui::success(format!("updated spice-pm -> {tag}; the new version runs on the next invocation"));
            Ok(())
        }
        Err(err) => {
            // put the previous binary back; best effort, nothing else to do
            let _ = std::fs::rename(&staged, &exe);
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

async fn download_script(http: &HttpClient, tag: &str) -> Result<Vec<u8>> {
    let spinner = ui::spinner(false, "downloading the install script");
    let bytes = http.download(&install_script_raw_url(tag)).await?;
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }
    // guard against writing an error page instead of a script
    if bytes.first().is_none_or(|b| *b != b'#') {
        return Err(Error::other(
            "downloaded installer does not look like a script; refusing to run it",
        ));
    }
    Ok(bytes)
}

fn write_temp_script(bytes: &[u8], tag: &str) -> Result<PathBuf> {
    let ext = if cfg!(windows) { "ps1" } else { "sh" };
    let path = std::env::temp_dir().join(format!(
        "spicepm-install-{}-{tag}.{ext}",
        std::process::id()
    ));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

fn run_installer(script: &Path, tag: &str, dir: &Path) -> Result<()> {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("powershell");
        c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
        c.arg(script).arg("-Version").arg(tag).arg("-InstallDir").arg(dir);
        c
    } else {
        let mut c = Command::new("bash");
        c.arg(script).arg("--version").arg(tag).arg("--dir").arg(dir);
        c
    };

    match command.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => {
            let code = status
                .code()
                .map_or_else(|| "signal".to_owned(), |c| c.to_string());
            Err(Error::other(format!(
                "installer exited with {code}; your previous spice-pm was restored. \
                 Reinstall manually: cargo install --path ."
            )))
        }
        Err(e) => {
            let hint = if cfg!(windows) {
                String::new()
            } else {
                " (bash is required)".to_owned()
            };
            Err(Error::other(format!(
                "could not start the installer{hint}: {e}; your previous spice-pm was restored"
            )))
        }
    }
}

fn verdict_for(current: &str, latest: &str) -> Verdict {
    if current == latest {
        return Verdict::UpToDate;
    }
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => match c.cmp(&l) {
            Ordering::Less => Verdict::UpdateAvailable,
            // equal or locally newer (dev build): never downgrade silently
            Ordering::Equal | Ordering::Greater => Verdict::UpToDate,
        },
        // unparseable tags: strings differ, offer the update anyway
        _ => Verdict::UpdateAvailable,
    }
}

fn parse_version(tag: &str) -> Option<(u64, u64, u64)> {
    let v = tag.strip_prefix('v').unwrap_or(tag);
    let mut parts = v.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next()?.parse().ok()?;
    let patch = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags() {
        assert_eq!(parse_version("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(parse_version("0.2.10"), Some((0, 2, 10)));
        assert_eq!(parse_version("v1.2.3-beta"), None);
        assert_eq!(parse_version("release-3"), None);
        assert_eq!(parse_version("v1"), None);
    }

    #[test]
    fn verdict_matrix() {
        assert_eq!(verdict_for("0.1.0", "v0.1.0"), Verdict::UpToDate);
        assert_eq!(verdict_for("0.1.0", "v0.2.0"), Verdict::UpdateAvailable);
        assert_eq!(verdict_for("0.10.0", "v0.9.9"), Verdict::UpToDate);
        assert_eq!(verdict_for("0.2.0", "v0.1.9"), Verdict::UpToDate);
        assert_eq!(verdict_for("dev", "v0.2.0"), Verdict::UpdateAvailable);
    }

    #[test]
    fn script_urls_are_pinned_to_tag() {
        let url = install_script_raw_url("v1.2.3");
        assert!(url.contains("/v1.2.3/install."), "{url}");
        assert!(url.starts_with("https://raw.githubusercontent.com/"), "{url}");
    }
}
