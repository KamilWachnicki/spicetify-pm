//! Shell out to `spicetify apply` when `--apply` is passed.

use crate::ui;
use std::sync::atomic::{AtomicBool, Ordering};

static APPLY_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn set_flag() {
    APPLY_REQUESTED.store(true, Ordering::Relaxed);
}

pub fn requested() -> bool {
    APPLY_REQUESTED.load(Ordering::Relaxed)
}

pub fn run_spicetify_apply() {
    match std::process::Command::new("spicetify")
        .arg("apply")
        .status()
    {
        Ok(status) if status.success() => {
            ui::success("ran `spicetify apply`");
        }
        Ok(status) => {
            ui::warn(format!("`spicetify apply` exited with {status}"));
        }
        Err(err) => {
            ui::warn(format!(
                "could not run `spicetify apply` ({err}); run it manually"
            ));
        }
    }
}
