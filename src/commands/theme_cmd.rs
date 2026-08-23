use crate::errors::{Error, Result};
use crate::spicetify::dirs;
use crate::spicetify::ini::SpicetifyConfig;
use crate::spicetify::schemes::parse_ini;
use crate::ui;
use serde::Serialize;

#[derive(Serialize)]
struct CurrentOut {
    theme: Option<String>,
    scheme: Option<String>,
}

pub fn run_set(name: Option<String>, scheme: Option<&str>) -> Result<()> {
    dirs::require_spicetify_dir()?;
    let themes_dir = dirs::themes_dir();

    let folder = match name {
        Some(n) => n,
        None => pick_theme_folder(&themes_dir)?,
    };

    let theme_root = themes_dir.join(&folder);
    if !theme_root.is_dir() {
        return Err(Error::other(format!(
            "no theme folder `Themes/{folder}`; run `spice-pm install` first or check the name"
        )));
    }

    let scheme_names = read_scheme_names(&theme_root);
    let chosen = choose_scheme(&scheme_names, scheme)?;

    let mut cfg = SpicetifyConfig::load(dirs::config_file())?;
    cfg.set_string("Setting", "current_theme", &folder);
    cfg.set_string(
        "Setting",
        "color_scheme",
        chosen.as_deref().unwrap_or_default(),
    );
    // activating a theme only works when spicetify actually uses its files
    let mut notices: Vec<&'static str> = Vec::new();
    if theme_root.join("user.css").is_file() && cfg.enable_flag("inject_css") {
        notices.push("set inject_css=1 so the theme styles apply");
    }
    if theme_root.join("color.ini").is_file() && cfg.enable_flag("replace_colors") {
        notices.push("set replace_colors=1 so colour schemes apply");
    }
    if theme_root.join("assets").is_dir() && cfg.enable_flag("overwrite_assets") {
        notices.push("set overwrite_assets=1 so spicetify copies the theme's assets");
    }
    cfg.save()?;

    ui::success(format!("active theme: {}", ui::style_title(&folder)));
    for notice in notices {
        ui::info(notice);
    }
    if let Some(s) = chosen {
        println!("         colour scheme: {s}");
    }
    ui::reminder_apply(crate::commands::apply_hook::requested());
    Ok(())
}

pub fn run_scheme(scheme: &str) -> Result<()> {
    dirs::require_spicetify_dir()?;
    let cfg = SpicetifyConfig::load(dirs::config_file())?;
    let Some(theme) = cfg.current_theme().filter(|t| !t.is_empty()) else {
        return Err(Error::other(
            "no active theme; run `spice-pm theme set` first",
        ));
    };
    let names = read_scheme_names(&dirs::themes_dir().join(&theme));
    if !names.is_empty() && !names.iter().any(|n| n == scheme) {
        return Err(Error::other(format!(
            "scheme `{scheme}` not found in Themes/{theme}/color.ini (available: {})",
            names.join(", ")
        )));
    }
    let mut cfg = cfg;
    cfg.set_string("Setting", "color_scheme", scheme);
    cfg.save()?;
    ui::success(format!("colour scheme: {scheme} (theme: {theme})"));
    ui::reminder_apply(crate::commands::apply_hook::requested());
    Ok(())
}

pub fn run_current(json: bool) -> Result<()> {
    let cfg = SpicetifyConfig::load(dirs::config_file())?;
    let theme = cfg.current_theme().filter(|t| !t.is_empty());
    let scheme = cfg.color_scheme().filter(|s| !s.is_empty());
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&CurrentOut { theme, scheme })?
        );
        return Ok(());
    }
    match (theme, scheme) {
        (Some(t), Some(s)) => println!("{t} ({s})"),
        (Some(t), None) => println!("{t}"),
        _ => ui::info("no theme active"),
    }
    Ok(())
}

fn read_scheme_names(theme_root: &std::path::Path) -> Vec<String> {
    let ini_path = theme_root.join("color.ini");
    let Ok(text) = std::fs::read_to_string(&ini_path) else {
        return Vec::new();
    };
    parse_ini(&text).keys().cloned().collect()
}

fn choose_scheme(available: &[String], provided: Option<&str>) -> Result<Option<String>> {
    if available.is_empty() {
        if provided.is_some() {
            ui::warn("theme has no color.ini; ignoring scheme argument");
        }
        return Ok(None);
    }
    if let Some(p) = provided {
        if !available.iter().any(|n| n == p) {
            return Err(Error::other(format!(
                "scheme `{p}` not found (available: {})",
                available.join(", ")
            )));
        }
        return Ok(Some(p.to_owned()));
    }
    if available.len() == 1 {
        return Ok(Some(available[0].clone()));
    }
    if console::Term::stderr().features().is_attended() {
        let refs: Vec<&str> = available.iter().map(String::as_str).collect();
        if let Ok(idx) = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Choose a colour scheme")
            .items(&refs)
            .default(0)
            .interact()
        {
            return Ok(Some(available[idx].clone()));
        }
    }
    Ok(Some(available[0].clone()))
}

fn pick_theme_folder(themes_dir: &std::path::Path) -> Result<String> {
    let mut folders: Vec<String> = std::fs::read_dir(themes_dir)?
        .filter_map(std::result::Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    folders.sort();
    if folders.is_empty() {
        return Err(Error::other("Themes/ has no theme folders"));
    }
    if !console::Term::stderr().features().is_attended() {
        return Err(Error::other(format!(
            "pass a theme name: {}",
            folders.join(", ")
        )));
    }
    let refs: Vec<&str> = folders.iter().map(String::as_str).collect();
    let idx = dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
        .with_prompt("Activate which theme?")
        .items(&refs)
        .default(0)
        .interact()
        .map_err(|e| Error::other(format!("selection cancelled: {e}")))?;
    Ok(folders.remove(idx))
}
