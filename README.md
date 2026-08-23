# spice-pm

A Spicetify **Marketplace package manager** for your terminal - discover,
install, update, and remove themes, extensions, and CSS snippets using the
**same sources, fetching logic, and filtering rules as the official
[Spicetify Marketplace](https://github.com/spicetify/marketplace)**.

Built in Rust. Linux-first; also runs on macOS and Windows.
Single static binary, no runtime dependencies beyond `spicetify` itself.

---

## Highlights

- **Real marketplace catalogue** - GitHub topic search over
  `spicetify-extensions` / `spicetify-themes`, official blacklist + archived
  filtering, manifest validation identical to the web app (invalid entries are
  skipped exactly where the app skips them)
- **Interactive pager** - 10 results per page with keyboard navigation, live
  `STATUS`/`ENABLED` columns, and digit keys that install/remove/toggle
  rows in place
- **Safe by default** - every action prints an exact summary of what will be
  written/deleted *before* it happens; destructive moves require confirmation
- **Lockfile** - snapshot your installed set and restore it anywhere with one
  command (`spice-pm lock` → zero-arg `spice-pm install`)
- **Clean reinstalls** - theme updates wipe the theme folder and reinstall
  from scratch (with local-drift detection), so you always match upstream

## Install

One-liners that fetch the latest release binary (checksum-verified when the
release ships a `.sha256` sidecar), install into your user bin/PATH, and print
next steps:

Linux / macOS:

```sh
curl -fsSL https://raw.githubusercontent.com/KamilWachnicki/spicetify-pm/main/install.sh | bash
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/KamilWachnicki/spicetify-pm/main/install.ps1 | iex
```

Both scripts accept `--version vX.Y.Z` / `-Version vX.Y.Z` to pin a release,
and `--dir` / `-InstallDir` to change the target directory. Release assets are
named `spice-pm-<tag>-<arch>-<os>.tar.gz|.zip`.

Build from source instead:

```sh
cargo install --path .        # or: cargo build --release
```

Requires [spicetify](https://spicetify.app) to already be set up. The config
directory is discovered the same way the spicetify CLI does it:
`SPICETIFY_CONFIG` env var → `%APPDATA%\spicetify` (Windows) /
`$XDG_CONFIG_HOME|~/.config` + `/spicetify` (Linux & macOS).

### GitHub token (recommended)

Unauthenticated GitHub API calls are limited to 60/hour. Export a token to
raise the limit - checked in order:

```fish
set -x GITHUB_TOKEN ghp_xxxx     # fish
export GITHUB_TOKEN=ghp_xxxx    # bash/zsh
```

`GH_TOKEN` and `SPICEPM_GITHUB_TOKEN` are also accepted.

---

## Usage

```
spice-pm search [query] [options]
spice-pm info <user/repo|url>
spice-pm install [<user/repo[#name]|url>] [--lockfile <path>]
spice-pm remove <id|name|file> [--yes]
spice-pm update [target]
spice-pm list [all|themes|extensions|snippets] [--json]
spice-pm lock [--out <path>]
spice-pm snippets search [query]|show|add|remove
spice-pm theme set [name] [scheme] | theme scheme <scheme> | theme current
spice-pm cache path|size|clear
spice-pm self-update [--check] [--yes]
```

Global flags: `--no-cache`, `--apply`, `--bypass-admin`, `-v/-vv` logging,
`--json` on read commands.

### Running as administrator/root

spice-pm **refuses to run elevated** (effective UID 0 on Linux/macOS, an
elevated token on Windows) - elevated sessions leave admin-owned files that
break spicetify for the regular user later, the same reason the spicetify CLI
itself guards this. Override when you genuinely need it:

```sh
sudo spice-pm search --bypass-admin
```

### search

```sh
spice-pm search --type theme --sort stars          # whole themes catalogue
spice-pm search bloom --type theme                 # substring filter
spice-pm search --json adblock                     # machine-readable output
spice-pm search --page 1 --sort a-z                # single API page
spice-pm search --archived                         # include archived repos
```

Every result row is an **individual manifest entry** - a multi-extension repo
like `rxri/spicetify-extensions` lists each extension separately, matching how
the marketplace grid renders cards. Repos without any valid manifest are
hidden. Columns: `# TITLE TYPE STATUS STARS DESCRIPTION`.

On a TTY the list becomes an interactive pager:

| Key | Action |
|---|---|
| `←`/`→` or `p`/`n` | page |
| `g`/`G` | first/last page |
| `0`–`9` | act on that row (see below) |
| `q`/`Esc`/`Ctrl+C` | quit |

What a digit press does:

- **uninstalled extension** → install summary → confirm (`proceed with
  install?`) → installs; the pager stays open on the same page so you can keep
  picking
- **installed extension** → removal summary → confirm removal; toggling both
  ways is fully reversible from the keyboard
- **uninstalled theme** → full file-by-file install summary → confirm →
  installs, colour scheme chosen interactively when several exist; the pager
  closes after one theme operation
- **installed theme** → removal summary → confirm removal; deactivates and
  clears `current_theme`/`color_scheme` only if that exact theme is active

Rows already installed show a green `✔ installed` status; enabled snippets
show green keys. Colors respect `NO_COLOR` and disappear when piping.

Piped output and `--json` always print the complete result set without prompts;
`--page N` limits fetching to a single GitHub results page.

### info

```sh
spice-pm info Comfy-Themes/Spicetify
spice-pm info https://github.com/rxri/spicetify-extensions --json
```

Shows repo metadata plus every valid manifest (id, authors, branch, tags,
download URLs) after the same blacklist check used by install.

### install / remove / update

```sh
spice-pm install rxri/spicetify-extensions#adblockify
spice-pm install Comfy-Themes/Spicetify#Comfy      # scheme prompt if needed
spice-pm install https://github.com/someone/some-theme
spice-pm remove adblock                            # fragment match + y/N
spice-pm remove adblock --yes                      # skip confirmation
spice-pm update StarryNight                         # clean-reinstall one item
spice-pm update                                     # everything
```

- **Extensions** land in `<SPICETIFY_CONFIG>/Extensions/<file>` and are
  registered under `[AdditionalOptions] extensions`.
- **Themes** land in `Themes/<name>/` - `user.css`, `color.ini`, every
  `include[]` file, and the first JS include is bridged to `theme.js` so
  spicetify auto-injects it. `current_theme` + your chosen `color_scheme` are
  written to `[Setting]`. If the theme ships scripts, spice-pm also sets
  `inject_theme_js=1` for you.
- **Theme updates are clean reinstalls**: the folder is wiped and rebuilt from
  upstream, local drift (edits/orphans) is detected and reported, and your
  previously selected colour scheme is restored when it still exists.
- After mutating commands spice-pm prints `run "spicetify apply"`; pass global
  `--apply` to run it for you.

### Lockfile

```sh
spice-pm lock                       # write <SPICETIFY_CONFIG>/spicepm/spicepm.lock
spice-pm lock --out ~/dotfiles/spicepm.lock
cd ~/dotfiles && spice-pm install   # restore everything, schemes included
```

The lockfile records each pinned item (kind, id, user/repo, branch, chosen
colour scheme) plus enabled snippet keys. It is **auto-refreshed on every
install/remove/update**, so it never drifts from reality. Zero-arg install
resolves `--lockfile` → `./spicepm.lock` → error with guidance.

### snippets

```sh
spice-pm snippets search            # interactive pager, digits toggle
spice-pm snippets search dancing    # filter by substring
spice-pm snippets add "Hamsters Dancing"
spice-pm snippets remove "Hamsters Dancing"
spice-pm snippets show "Sonic Dancing"
spice-pm list snippets              # enabled snippet keys
```

Enabled snippets are applied through a generated companion extension
(`Extensions/spicepm-snippets.js`) that injects them at runtime - the same
mechanism the marketplace app uses, surviving theme switches with zero theme
file pollution. The companion is rebuilt automatically whenever extensions are
installed/removed, and orphaned files in `Extensions/` are cleaned up.

### theme

```sh
spice-pm theme set                  # pick from installed themes interactively
spice-pm theme set Cattpuccin mocha
spice-pm theme scheme latte
spice-pm theme current --json
```

### cache

Responses are cached on disk with per-type TTLs (search 10 min, manifests and
repo metadata 24 h, blacklist/snippets 1 h). Expiring entries are revalidated
with their ETag - `304` answers cost no GitHub rate-limit quota. When a
refresh fails (rate limit, offline), a stale cached copy is served with a
warning instead of failing; `--no-cache` disables all of that. Entries older
than 30 days are pruned automatically.

```sh
spice-pm cache path                 # print the cache directory
spice-pm cache size                 # entry count + total size
spice-pm cache clear
```

### self-update

Compares the running version against the latest GitHub release and, when
outdated, re-runs the install script pinned to that tag over the current
binary in place (the previous binary is restored if anything fails).

```sh
spice-pm self-update                # confirm + update
spice-pm self-update --yes          # skip confirmation
spice-pm self-update --check        # compare only; exit 1 when outdated
```

---

## How it behaves

- **Identity**: every item gets a unique, meaningful id -
  `{user}/{repo}#{Manifest Name}` (e.g. `Comfy-Themes/Spicetify#Comfy`) that
  matches the install target syntax; the snippet companion lives at reserved id
  `@spicepm/snippets`.
- **Ledger**: `<SPICETIFY_CONFIG>/spicepm/ledger.json` tracks provenance
  (source, branch, resolved URLs, sha256 per file, config references). This is
  what powers `update`, exact removals, and `STATUS` marks.
- **Atomicity**: config and ledger writes go through temp-file renames; failed
  actions leave the previous state intact.
- **Rate limits**: exhausted quota produces a clear message with the reset
  time; retries cover transient network/server errors.
- **Safety**: paths recorded in the ledger cannot escape the spicetify dir;
  downloads overwrite atomically; two items can't claim the same extension
  filename.

## Marketplace compliance

`spice-pm` ports the marketplace's remote logic field by field:

| [Publishing rule](https://github.com/spicetify/marketplace/wiki/Publishing-to-Marketplace) | spice-pm |
|---|---|
| Discovery via `spicetify-extensions` / `spicetify-themes` topics | ✅ topic search, `per_page=100`, paginated |
| `manifest.json` in repo root | ✅ fetched from raw + default branch |
| Array manifests (multi-extension repos) | ✅ every entry expanded individually |
| Field requirements/optionality | ✅ mirrors the app's zod schema |
| `branch` override / default fallback | ✅ |
| Authors fallback to repo owner; URL sanitization | ✅ |
| http(s) URL support for `preview/main/readme/usercss/schemes` | ✅ verbatim vs raw-relative resolution |
| Blacklist + archived filtering | ✅ glob semantics identical (`*` = one path segment) |
| Theme `schemes` | ✅ parsed & offered at install |
| Theme `include[]` scripts | ✅ downloaded **and** bridged to `theme.js`; `inject_theme_js` enabled automatically |
| Snippets from `resources/snippets.json` | ✅ via companion extension |
| Custom apps (`spicetify-apps`) | ⏳ planned milestone |

## Development

```sh
cargo test                    # 66 unit tests
cargo clippy --all-targets    # zero warnings (pedantic lints, unsafe forbidden)
cargo fmt --check
cargo build --release
```

Layout of interest:

| Path | Responsibility |
|---|---|
| `src/market/` | marketplace parity: search, manifests (zod-parity validation), blacklist globs, snippet fetch, URL rules |
| `src/spicetify/` | spicetify CLI parity: directory layout, `config-xpui.ini` editing, `color.ini` parsing |
| `src/commands/` | one module per command group + shared pager plumbing |
| `src/http.rs`, `src/cache.rs` | client (token, retries, rate-limit reporting) + TTL disk cache |
| `src/ledger.rs` | installed-state tracking (ids, hashes, provenance) |

Environment: `RUST_LOG=-v` equivalent via flags; `NO_COLOR` respected;
`SPICETIFY_CACHE` overrides the cache dir.

Roadmap: custom apps support (`topic:spicetify-apps`), shell completions, cargo-dist release pipeline.

## License

LGPL 2.1
