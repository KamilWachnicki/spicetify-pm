use crate::cli::{parse_target, split_manifest_selector};
use crate::commands::load_blacklist;
use crate::errors::{Error, Result};
use crate::http::HttpClient;
use crate::ledger::{self, Kind, Ledger};
use crate::market::items::{build_card_items, get_repo_manifests};
use crate::market::search::get_repo;
use crate::market::types::CardItem;
use crate::spicetify::dirs;
use crate::spicetify::ini::SpicetifyConfig;
use crate::ui;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub async fn run(
    http: &HttpClient,
    target: Option<String>,
    yes: bool,
    lockfile_flag: Option<PathBuf>,
) -> Result<()> {
    match target {
        Some(t) => install_target(http, &t, yes).await,
        None => install_from_lockfile(http, lockfile_flag.as_deref()).await,
    }
}

/// Zero-arg mode: bring this machine to the state captured in spicepm.lock.
#[allow(clippy::too_many_lines)]
async fn install_from_lockfile(
    http: &HttpClient,
    lockfile_flag: Option<&std::path::Path>,
) -> Result<()> {
    dirs::require_spicetify_dir()?;
    let path = crate::lockfile::resolve_install_path(lockfile_flag)?;
    let lock = crate::lockfile::Lockfile::load(&path)?;
    ui::info(format!(
        "installing from {} ({} item(s), {} snippet(s))",
        ui::style_title(&path.display().to_string()),
        lock.items.len(),
        lock.snippets.len()
    ));

    let all_snippets = if lock.snippets.is_empty() {
        None
    } else {
        Some(crate::market::snippets::fetch_snippets(http).await?)
    };

    let mut done = 0usize;
    for lock_item in &lock.items {
        let repo = crate::market::search::get_repo(http, &lock_item.user, &lock_item.repo).await?;
        let (manifests, warnings) =
            get_repo_manifests(http, &lock_item.user, &lock_item.repo, &repo.default_branch)
                .await?;
        for warning in warnings {
            ui::warn(warning);
        }
        let items = build_card_items(&repo, manifests);
        let kind_matches = |c: &CardItem| {
            matches!(
                (lock_item.kind, c.kind),
                (
                    crate::ledger::Kind::Extension,
                    crate::market::types::ItemKind::Extension
                ) | (
                    crate::ledger::Kind::Theme,
                    crate::market::types::ItemKind::Theme
                )
            )
        };
        let Some(card) = items
            .iter()
            .find(|c| c.id() == lock_item.id && kind_matches(c))
        else {
            ui::warn(format!(
                "`{}` no longer exists upstream; skipping",
                lock_item.id
            ));
            continue;
        };

        println!();
        ui::info(format!(
            "installing {} [{}] ...",
            ui::style_title(&card.title),
            card.id()
        ));
        // non-interactive: prompts suppressed, locked scheme applied below
        install_item(http, card, true).await?;
        done += 1;

        if lock_item.kind == crate::ledger::Kind::Theme
            && let Some(wanted) = &lock_item.scheme
        {
            restore_scheme(card, wanted)?;
        }
    }

    if !lock.snippets.is_empty() {
        let all = all_snippets.unwrap_or_default();
        let mut led = Ledger::load()?;
        for key in &lock.snippets {
            super::snippets::enable_key(&mut led, &all, key)?;
        }
        crate::commands::persist_ledger(&mut led)?;
    }

    println!();
    ui::success(format!(
        "lockfile install complete: {done} item(s), {} snippet(s)",
        lock.snippets.len()
    ));
    ui::reminder_apply(std::env::args().any(|a| a == "--apply"));
    Ok(())
}

/// Re-apply a locked colour scheme when it still exists in the freshly
/// installed theme.
fn restore_scheme(card: &CardItem, wanted: &str) -> Result<()> {
    let folder = sanitize_folder_name(&card.manifest.name);
    let ini_path = dirs::themes_dir().join(&folder).join("color.ini");
    let Ok(text) = std::fs::read_to_string(&ini_path) else {
        return Ok(());
    };
    let available: Vec<String> = crate::spicetify::schemes::parse_ini(&text)
        .keys()
        .cloned()
        .collect();
    if let Some(scheme) = crate::spicetify::schemes::preferred_scheme(Some(wanted), &available) {
        let mut cfg = SpicetifyConfig::load(dirs::config_file())?;
        if cfg.color_scheme().as_deref() != Some(scheme.as_str()) {
            cfg.set_string("Setting", "color_scheme", &scheme);
            cfg.save()?;
            println!("         colour scheme restored: {scheme}");
        }
    }
    Ok(())
}

async fn install_target(http: &HttpClient, target: &str, yes: bool) -> Result<()> {
    dirs::require_spicetify_dir()?;
    let (base, selector) = split_manifest_selector(target);
    let (user, repo_name) = parse_target(base)
        .ok_or_else(|| Error::other(format!("cannot parse `{base}` as user/repo")))?;

    let spinner = ui::spinner(false, format!("fetching {user}/{repo_name}"));
    let repo = get_repo(http, &user, &repo_name).await?;
    let blacklist = load_blacklist(http).await?;
    if blacklist.is_blacklisted(&repo.html_url) {
        return Err(Error::other(format!(
            "{user}/{repo_name} is blacklisted by the official marketplace and will not be installed"
        )));
    }
    let owner = repo.full_name.split('/').next().unwrap_or(&user).to_owned();
    let (manifests, warnings) =
        get_repo_manifests(http, &owner, &repo.name, &repo.default_branch).await?;
    for warning in &warnings {
        ui::warn(warning);
    }
    let mut items = build_card_items(&repo, manifests);
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    // optional `#manifest-name` selector (case-insensitive on manifest.name)
    if let Some(sel) = selector {
        let lower = sel.to_lowercase();
        items.retain(|i| i.manifest.name.to_lowercase() == lower);
        if items.is_empty() {
            return Err(Error::other(format!(
                "no manifest named `{sel}` in {user}/{repo_name}"
            )));
        }
    }

    let item = pick_item(&items, yes)?;
    install_item(http, item, yes).await?;
    ui::reminder_apply(crate::commands::apply_hook::requested());
    Ok(())
}

/// Install a fully resolved `CardItem` - shared by the `install` command and
/// search's digit-key quick-install.
pub(crate) async fn install_item(http: &HttpClient, item: &CardItem, yes: bool) -> Result<()> {
    match item.kind {
        crate::market::types::ItemKind::Extension => install_extension(http, item).await,
        crate::market::types::ItemKind::Theme => install_theme(http, item, yes).await,
    }
}

fn pick_item(items: &[CardItem], yes: bool) -> Result<&CardItem> {
    match items.len() {
        0 => Err(Error::other(
            "no installable extension/theme manifests found",
        )),
        1 => Ok(&items[0]),
        _ if yes => Err(Error::other(
            "multiple manifests found; disambiguate with `user/repo#manifest-name`",
        )),
        _ => {
            if !console::Term::stderr().features().is_attended() {
                return Err(Error::other(
                    "multiple manifests found and no TTY; use `user/repo#manifest-name`",
                ));
            }
            let labels: Vec<String> = items
                .iter()
                .map(|i| format!("[{}] {}", kind_label(i.kind), i.title))
                .collect();
            let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let selection =
                dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
                    .with_prompt("Multiple manifests found - which one?")
                    .items(&label_refs)
                    .default(0)
                    .interact()
                    .map_err(|e| Error::other(format!("selection cancelled: {e}")))?;
            Ok(&items[selection])
        }
    }
}

fn kind_label(kind: crate::market::types::ItemKind) -> &'static str {
    match kind {
        crate::market::types::ItemKind::Extension => "extension",
        crate::market::types::ItemKind::Theme => "theme",
    }
}

/// Last path segment of a manifest `main`/`usercss` value, whether it is a
/// relative path or an absolute URL.
pub fn file_name_from(value: &str) -> String {
    let cleaned = value.split(['?', '#']).next().unwrap_or(value);
    cleaned
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or(cleaned)
        .to_owned()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

async fn install_extension(http: &HttpClient, item: &CardItem) -> Result<()> {
    let url = item
        .extension_url
        .clone()
        .ok_or_else(|| Error::other("manifest has no downloadable main"))?;
    let basename = file_name_from(item.manifest.main.as_deref().unwrap_or_default());
    if basename.is_empty() {
        return Err(Error::other("cannot determine extension filename"));
    }

    let spinner = ui::spinner(false, format!("downloading {basename}"));
    let bytes = http.download(&url).await?;
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    let mut cfg = SpicetifyConfig::load(dirs::config_file())?;
    let mut led = Ledger::load()?;

    let dest = dirs::extensions_dir().join(&basename);
    std::fs::create_dir_all(dirs::extensions_dir())?;
    let rel = ledger::rel_from_base(&dest);

    // guard against two different ids owning the same filename
    if let Some(existing) = led.entries.iter().find(|e| e.files.contains(&rel))
        && existing.id != item.id()
    {
        return Err(Error::other(format!(
            "`{basename}` is already owned by `{}`; remove it first",
            existing.id
        )));
    }

    ledger::resolve_relative(&rel)?;
    crate::cache::atomic_write(&dest, &bytes)?;

    cfg.list_add("extensions", &basename);

    led.upsert(crate::ledger::LedgerEntry {
        id: item.id(),
        kind: Kind::Extension,
        user: item.user.clone(),
        repo: item.repo.clone(),
        branch: item.branch.clone(),
        files: vec![rel.clone()],
        config_refs: vec![basename.clone()],
        resolved_urls: vec![url.clone()],
        hashes: [(rel, ledger::hash_bytes(&bytes))].into_iter().collect(),
        installed_at: now_secs(),
        ..Default::default()
    });
    cfg.save()?;
    crate::commands::persist_ledger(&mut led)?;
    cfg.save()?;
    crate::commands::reconcile_extensions_dir(&led)?;
    crate::commands::snippets::rebuild_companion(http).await?;

    ui::success(format!(
        "installed extension {} ({})",
        ui::style_title(&item.title),
        basename
    ));
    Ok(())
}

#[allow(clippy::too_many_lines)]
async fn install_theme(http: &HttpClient, item: &CardItem, yes: bool) -> Result<()> {
    let css_url = item
        .css_url
        .clone()
        .ok_or_else(|| Error::other("manifest has no usercss"))?;

    let spinner = ui::spinner(false, "downloading theme files");
    let css = http.download(&css_url).await?;
    let schemes = match &item.schemes_url {
        Some(u) => Some((u.clone(), http.download(u).await?)),
        None => None,
    };

    // include[] files keep their relative layout so url(...) assets resolve;
    // absolute-URL entries store under their filename
    let mut include_files: Vec<(String, String, Vec<u8>)> = Vec::new(); // (dest_rel, url, bytes)
    for inc in &item.manifest.include {
        let url = crate::market::urls::resolve_or_raw(inc, &item.user, &item.repo, &item.branch);
        let bytes = http.download(&url).await?;
        let rel = include_dest_rel(inc)?;
        include_files.push((rel, url.clone(), bytes));
    }
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    // pick a unique folder name from the manifest name; reinstalling the
    // exact same theme reuses its original folder
    let base_slug = sanitize_folder_name(&item.manifest.name);
    let themes_dir = dirs::themes_dir();
    std::fs::create_dir_all(&themes_dir)?;
    let prior = Ledger::load()?
        .find(&item.id())
        .and_then(|e| e.config_refs.first().cloned().filter(|f| !f.is_empty()));
    let folder = match prior {
        Some(f) if !themes_dir.join(&f).exists() || is_own_theme_folder(&item.id(), &f) => f,
        _ => {
            let mut folder = base_slug.clone();
            let mut counter = 2u32;
            while !is_own_theme_folder(&item.id(), &folder) && themes_dir.join(&folder).exists() {
                folder = format!("{base_slug}-{counter}");
                counter += 1;
            }
            folder
        }
    };
    let theme_root = themes_dir.join(&folder);

    let mut files: Vec<(std::path::PathBuf, Vec<u8>)> = vec![(theme_root.join("user.css"), css)];
    if let Some((_, ini_bytes)) = &schemes {
        files.push((theme_root.join("color.ini"), ini_bytes.clone()));
    }
    for (rel_inc, _, bytes) in &include_files {
        let safe = safe_join(&theme_root, rel_inc)?;
        files.push((safe, bytes.clone()));
    }
    // marketplace themes can ship scripts via include[]; spicetify only
    // auto-injects theme.js, so bridge the first JS include to that name
    let include_pairs: Vec<(String, Vec<u8>)> = include_files
        .iter()
        .map(|(rel, _, bytes)| (rel.clone(), bytes.clone()))
        .collect();
    let mut ships_script = false;
    if let Some((theme_js, bytes)) = theme_js_bridge(&theme_root, &include_pairs) {
        files.push((theme_js, bytes));
        ships_script = true;
    } else if include_files
        .iter()
        .any(|(rel, _, _)| rel.to_lowercase() == "theme.js")
    {
        // include already named theme.js: spicetify injects it as-is
        ships_script = true;
    }
    for (path, bytes) in &files {
        let rel = ledger::rel_from_base(path);
        ledger::resolve_relative(&rel)?;
        crate::cache::atomic_write(path, bytes)?;
    }

    // colour scheme choice
    let scheme_names: Vec<String> = schemes
        .as_ref()
        .map(|(_, bytes)| {
            let text = String::from_utf8_lossy(bytes);
            crate::spicetify::schemes::parse_ini(&text)
                .keys()
                .cloned()
                .collect()
        })
        .unwrap_or_default();

    let chosen_scheme: Option<String> = if scheme_names.is_empty() {
        None
    } else if scheme_names.len() == 1 || yes || !console::Term::stderr().features().is_attended() {
        Some(scheme_names[0].clone())
    } else {
        let refs: Vec<&str> = scheme_names.iter().map(String::as_str).collect();
        match dialoguer::Select::with_theme(&dialoguer::theme::ColorfulTheme::default())
            .with_prompt("Choose a colour scheme")
            .items(&refs)
            .default(0)
            .interact()
        {
            Ok(idx) => Some(scheme_names[idx].clone()),
            Err(_) => None,
        }
    };

    let mut cfg = SpicetifyConfig::load(dirs::config_file())?;
    cfg.set_string("Setting", "current_theme", &folder);
    match &chosen_scheme {
        Some(s) => cfg.set_string("Setting", "color_scheme", s),
        None => cfg.set_string("Setting", "color_scheme", ""),
    }
    // the written files are useless unless spicetify actually uses them
    let enabled_css = cfg.enable_flag("inject_css");
    let enabled_colors = if schemes.is_some() {
        cfg.enable_flag("replace_colors")
    } else {
        false
    };
    let enabled_inject = if ships_script {
        cfg.enable_flag("inject_theme_js")
    } else {
        false
    };

    let mut led = Ledger::load()?;
    let rel_files: Vec<String> = files
        .iter()
        .map(|(p, _)| ledger::rel_from_base(p))
        .collect();
    let hashes = files
        .iter()
        .map(|(p, b)| (ledger::rel_from_base(p), ledger::hash_bytes(b)))
        .collect();

    led.upsert(crate::ledger::LedgerEntry {
        id: item.id(),
        kind: Kind::Theme,
        user: item.user.clone(),
        repo: item.repo.clone(),
        branch: item.branch.clone(),
        files: rel_files,
        config_refs: vec![folder.clone()],
        resolved_urls: {
            let mut urls = vec![css_url];
            if let Some((u, _)) = &schemes {
                urls.push(u.clone());
            }
            urls.extend(include_files.iter().map(|(_, u, _)| u.clone()));
            urls
        },
        hashes,
        installed_at: now_secs(),
        ..Default::default()
    });
    cfg.save()?;
    crate::commands::persist_ledger(&mut led)?;

    ui::success(format!(
        "installed theme {} into Themes/{folder}",
        ui::style_title(&item.manifest.name)
    ));
    if let Some(s) = &chosen_scheme {
        println!("         colour scheme: {s}");
    } else {
        ui::info("no colour schemes found in this theme");
    }
    if enabled_css {
        ui::info("set inject_css=1 so the theme styles apply");
    }
    if enabled_colors {
        ui::info("set replace_colors=1 so colour schemes apply");
    }
    if enabled_inject {
        ui::info("set inject_theme_js=1 so the theme script runs");
    }
    Ok(())
}

/// True when the ledger says this theme id owns `Themes/<folder>`.
fn is_own_theme_folder(id: &str, folder: &str) -> bool {
    match Ledger::load() {
        Ok(led) => led.entries.iter().any(|e| {
            e.kind == Kind::Theme && e.id == id && e.config_refs.iter().any(|r| r == folder)
        }),
        Err(_) => false,
    }
}

/// Destination (relative to the theme folder) for a manifest `include[]`
/// entry: absolute URLs store under their filename; relative entries keep
/// their full layout so `url(...)` assets resolve.
pub fn include_dest_rel(inc: &str) -> Result<String> {
    if inc.starts_with("http") {
        let name = file_name_from(inc);
        if name.is_empty() {
            return Err(Error::other(format!(
                "cannot determine filename for include `{inc}`"
            )));
        }
        Ok(name)
    } else {
        let normalized = inc.replace('\\', "/");
        let mut parts = Vec::new();
        for comp in normalized.split('/') {
            if comp.is_empty() || comp == "." {
                continue;
            }
            if comp == ".." {
                return Err(Error::other(format!(
                    "include path `{inc}` escapes the theme folder"
                )));
            }
            parts.push(comp);
        }
        if parts.is_empty() {
            return Err(Error::other(format!("empty include path `{inc}`")));
        }
        Ok(parts.join("/"))
    }
}

/// The marketplace ships theme scripts via `include[]` and injects them at
/// runtime; on disk, spicetify only auto-injects `Themes/<name>/theme.js`.
/// Bridge the two: if any include is JavaScript and none is already named
/// `theme.js`, return the first one for writing under the magic name.
pub(crate) fn theme_js_bridge(
    theme_root: &std::path::Path,
    include_files: &[(String, Vec<u8>)],
) -> Option<(std::path::PathBuf, Vec<u8>)> {
    let mut first_js: Option<(String, Vec<u8>)> = None;
    for (rel, bytes) in include_files {
        if !rel.to_lowercase().ends_with(".js") {
            continue;
        }
        if rel.eq_ignore_ascii_case("theme.js") {
            return None; // already installed under the magic name
        }
        first_js.get_or_insert_with(|| (rel.clone(), bytes.clone()));
    }
    first_js.map(|(_, bytes)| (theme_root.join("theme.js"), bytes))
}

pub(crate) fn sanitize_folder_name(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let trimmed = slug.trim();
    if trimmed.is_empty() {
        "unnamed-theme".to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// Join an include[] relative path onto a root, refusing traversal outside it.
fn safe_join(root: &std::path::Path, rel: &str) -> Result<std::path::PathBuf> {
    let mut out = root.to_path_buf();
    for comp in rel.replace('\\', "/").split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp == ".." {
            return Err(Error::other(format!(
                "include path `{rel}` escapes the theme folder"
            )));
        }
        out.push(comp);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_filenames() {
        assert_eq!(file_name_from("dist/foo.js"), "foo.js");
        assert_eq!(file_name_from("https://x.com/a/b/bar.css?raw=1"), "bar.css");
        assert_eq!(file_name_from("single"), "single");
    }

    #[test]
    fn sanitizes_folders() {
        assert_eq!(sanitize_folder_name("My Cool/Theme"), "My Cool-Theme");
        assert_eq!(sanitize_folder_name("  "), "unnamed-theme");
    }

    #[test]
    fn rejects_traversal_includes() {
        let root = std::path::Path::new("/tmp/x");
        assert!(safe_join(root, "../evil").is_err());
        assert_eq!(
            safe_join(root, "assets/font.ttf").unwrap(),
            root.join("assets/font.ttf")
        );
    }

    #[test]
    fn include_destinations() {
        // absolute URLs store under their filename, not a URL-shaped path
        assert_eq!(
            include_dest_rel("https://comfy-themes.github.io/Spicetify/Comfy/theme.script.js")
                .unwrap(),
            "theme.script.js"
        );
        // relative entries keep their layout
        assert_eq!(
            include_dest_rel("./assets/fonts/Inter.ttf").unwrap(),
            "assets/fonts/Inter.ttf"
        );
        assert!(include_dest_rel("../escape.js").is_err());
    }

    #[test]
    fn theme_js_bridge_semantics() {
        let root = std::path::Path::new("/tmp/t");
        let js = |name: &str, body: &[u8]| (name.to_owned(), body.to_vec());

        // JS include gets bridged to theme.js
        let bridge = theme_js_bridge(root, &[js("theme.script.js", b"abc")]).unwrap();
        assert_eq!(bridge.0, root.join("theme.js"));
        assert_eq!(bridge.1, b"abc");

        // an include already named theme.js needs no bridging
        assert!(theme_js_bridge(root, &[js("theme.js", b"abc")]).is_none());

        // non-JS includes never bridge
        assert!(theme_js_bridge(root, &[js("assets/font.ttf", b"x")]).is_none());

        // first JS include wins when several exist
        let bridge = theme_js_bridge(root, &[js("a.js", b"first"), js("b.js", b"second")]).unwrap();
        assert_eq!(bridge.1, b"first");
    }
}
