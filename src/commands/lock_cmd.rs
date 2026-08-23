//! `spicepm lock` - snapshot the installed set into the lockfile.

use crate::errors::Result;
use crate::ledger::Ledger;
use crate::lockfile::{Lockfile, default_path};
use crate::ui;
use std::path::PathBuf;

pub fn run(out: Option<PathBuf>) -> Result<()> {
    let led = Ledger::load()?;
    let snippets = crate::commands::snippets::enabled_keys(&led);
    let lockfile = Lockfile::from_ledger(&led, &snippets);

    let path = out.unwrap_or_else(default_path);
    lockfile.store(&path)?;

    ui::success(format!(
        "wrote {} ({} item(s), {} snippet(s))",
        ui::style_title(&path.display().to_string()),
        lockfile.items.len(),
        lockfile.snippets.len()
    ));
    Ok(())
}
