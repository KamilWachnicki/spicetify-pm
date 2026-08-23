//! `self-update`: compare the running version against the latest GitHub
//! release and, when outdated, re-run the platform install script pinned to
//! that release over this binary in place.

use crate::errors::{Error, Result};
use crate::http::HttpClient;
use crate::ui;
use serde::Deserialize;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// This tool's own repository, used by `self-update`.
const SELF_REPO: &str = "KamilWachnicki/spicetify-pm";

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
    let current = env!("CARGO_PKG_VERSION");
    let spinner = ui::spinner(check, "fetching the latest release");
    let release: LatestRelease = http.get_json(&latest_release_api_url()).await?;
    let latest = release.tag_name;
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    match verdict_for(current, &latest) {
        Verdict::UpToDate => {
            ui::success(format!(
                "spice-pm {current} is up to date (latest release {latest})"
            ));
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
        let confirmed = dialoguer::Confirm::with_theme(&dialoguer::theme::ColorfulTheme::default())
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

fn latest_release_api_url() -> String {
    format!("https://api.github.com/repos/{SELF_REPO}/releases/latest")
}

/// Everything platform-specific about the release installer, stated once:
/// which script a release ships, the extension a downloaded copy gets, and
/// how to invoke it - instead of a fresh `cfg!(windows)` fork at each site.
struct InstallerScript;

impl InstallerScript {
    fn file() -> &'static str {
        if cfg!(windows) {
            "install.ps1"
        } else {
            "install.sh"
        }
    }

    fn ext() -> &'static str {
        if cfg!(windows) { "ps1" } else { "sh" }
    }

    fn raw_url(tag: &str) -> String {
        format!(
            "https://raw.githubusercontent.com/{SELF_REPO}/{tag}/{}",
            Self::file()
        )
    }

    fn command(script: &Path, tag: &str, dir: &Path) -> Command {
        if cfg!(windows) {
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]);
            c.arg(script)
                .arg("-Version")
                .arg(tag)
                .arg("-InstallDir")
                .arg(dir);
            c
        } else {
            let mut c = Command::new("bash");
            c.arg(script)
                .arg("--version")
                .arg(tag)
                .arg("--dir")
                .arg(dir);
            c
        }
    }

    /// Hint appended when spawning fails (bash missing on unix).
    fn spawn_hint() -> &'static str {
        if cfg!(windows) {
            ""
        } else {
            " (bash is required)"
        }
    }
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
    let tmp = write_temp_script(&script_bytes)?;

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
            ui::success(format!(
                "updated spice-pm -> {tag}; the new version runs on the next invocation"
            ));
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
    let bytes = http.download(&InstallerScript::raw_url(tag)).await?;
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }
    // guard against writing an error page instead of a script: bash scripts
    // open with a shebang (`#!`), PowerShell ones with a comment block (`<#`)
    if !looks_like_installer(&bytes) {
        return Err(Error::other(
            "downloaded installer does not look like a script; refusing to run it",
        ));
    }
    Ok(bytes)
}

fn looks_like_installer(bytes: &[u8]) -> bool {
    bytes.starts_with(b"#!") || bytes.starts_with(b"<#")
}

fn write_temp_script(bytes: &[u8]) -> Result<PathBuf> {
    let mut seed = format!("spice-pm-install-{}", std::process::id());
    for attempt in 0..8u32 {
        // unpredictable name + exclusive creation: a local user cannot
        // pre-place a symlink at the path and redirect this write
        let _ = write!(seed, "-{attempt}-{}", now_nanos());
        let nonce = crate::ledger::hash_bytes(seed.as_bytes());
        let path = std::env::temp_dir().join(format!(
            "spice-pm-install-{nonce}.{}",
            InstallerScript::ext()
        ));
        match open_exclusive(&path) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::other("could not create a temporary install script"))
}

/// Open for writing only if the file does not exist yet (`O_EXCL`); private
/// to the current user on unix.
fn open_exclusive(path: &Path) -> std::io::Result<std::fs::File> {
    #[allow(unused_mut)]
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.mode(0o600);
    }
    opts.open(path)
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

fn run_installer(script: &Path, tag: &str, dir: &Path) -> Result<()> {
    match InstallerScript::command(script, tag, dir).status() {
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
        Err(e) => Err(Error::other(format!(
            "could not start the installer{}: {e}; your previous spice-pm was restored",
            InstallerScript::spawn_hint()
        ))),
    }
}

fn verdict_for(current: &str, latest: &str) -> Verdict {
    match (Version::parse(current), Version::parse(latest)) {
        // equal or locally newer (dev build): never downgrade silently
        (Some(c), Some(l)) if c >= l => Verdict::UpToDate,
        (Some(_), Some(_)) => Verdict::UpdateAvailable,
        // identical strings (e.g. two dev builds) are up to date; unparseable
        // differing tags offer the update anyway
        _ if current == latest => Verdict::UpToDate,
        _ => Verdict::UpdateAvailable,
    }
}

/// A parsed `major.minor.patch` version, ordered component-wise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Version(u64, u64, u64);

impl Version {
    /// Parse a version out of a release tag, tolerating a leading `v`.
    fn parse(tag: &str) -> Option<Self> {
        let rest = tag.strip_prefix('v').unwrap_or(tag);
        let mut nums = rest.split('.');
        let major = nums.next()?.parse().ok()?;
        let minor = nums.next()?.parse().ok()?;
        let patch = nums.next()?.parse().ok()?;
        (nums.next().is_none()).then_some(Self(major, minor, patch))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tags() {
        assert_eq!(Version::parse("v0.1.0"), Some(Version(0, 1, 0)));
        assert_eq!(Version::parse("0.2.10"), Some(Version(0, 2, 10)));
        assert_eq!(Version::parse("v1.2.3-beta"), None);
        assert_eq!(Version::parse("release-3"), None);
        assert_eq!(Version::parse("v1"), None);
    }

    #[test]
    fn verdict_matrix() {
        assert_eq!(verdict_for("0.1.0", "v0.1.0"), Verdict::UpToDate);
        assert_eq!(verdict_for("0.1.0", "v0.2.0"), Verdict::UpdateAvailable);
        assert_eq!(verdict_for("0.10.0", "v0.9.9"), Verdict::UpToDate);
        assert_eq!(verdict_for("0.2.0", "v0.1.9"), Verdict::UpToDate);
        assert_eq!(verdict_for("dev", "v0.2.0"), Verdict::UpdateAvailable);
        assert_eq!(verdict_for("dev", "dev"), Verdict::UpToDate);
    }

    #[test]
    fn installer_guard_accepts_both_platforms() {
        assert!(looks_like_installer(
            b"#!/usr/bin/env bash\nset -euo pipefail"
        ));
        assert!(looks_like_installer(
            b"<#\n.SYNOPSIS\n    spice-pm installer\n#>"
        ));
        assert!(!looks_like_installer(b"<html>404 not found</html>"));
        assert!(!looks_like_installer(b""));
        assert!(!looks_like_installer(b"# just a hash"));
    }

    #[test]
    fn script_urls_are_pinned_to_tag() {
        let url = InstallerScript::raw_url("v1.2.3");
        assert!(url.contains("/v1.2.3/install."), "{url}");
        assert!(
            url.starts_with("https://raw.githubusercontent.com/"),
            "{url}"
        );
    }
}
