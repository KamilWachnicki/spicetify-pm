# spicepm

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
  `STATUS`/`ENABLED` columns, and digit keys that install/uninstall/toggle
  rows in place
- **Safe by default** - every action prints an exact summary of what will be
  written/deleted *before* it happens; destructive moves require confirmation
- **Lockfile** - snapshot your installed set and restore it anywhere with one
  command (`spicepm lock` → zero-arg `spicepm install`)
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
named `spicepm-<tag>-<arch>-<os>.tar.gz|.zip`.

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
spicepm search [query] [options]
spicepm info <user/repo|url>
spicepm install [<user/repo[#name]|url>] [--lockfile <path>]
spicepm uninstall <id|name|file> [--yes]
spicepm update [target]
spicepm list installed
spicepm lock [--out <path>]
spicepm snippets list|show|add|remove|installed
spicepm theme set [name] [scheme] | theme scheme <scheme> | theme current
spicepm cache path|clear
```

Global flags: `--no-cache`, `--apply`, `--bypass-admin`, `-v/-vv` logging,
`--json` on read commands.

### Running as administrator/root

spicepm **refuses to run elevated** (effective UID 0 on Linux/macOS, an
elevated token on Windows) — elevated sessions leave admin-owned files that
break spicetify for the regular user later, the same reason the spicetify CLI
itself guards this. Override when you genuinely need it:

```sh
sudo spicepm search --bypass-admin
```

### search

```sh
spicepm search --type theme --sort stars          # whole themes catalogue
spicepm search bloom --type theme                 # substring filter
spicepm search --json adblock                     # machine-readable output
spicepm search --page 1 --sort a-z                # single API page
spicepm search --archived                         # include archived repos
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
- **installed extension** → removal summary → confirm uninstall; toggling both
  ways is fully reversible from the keyboard
- **uninstalled theme** → full file-by-file install summary → confirm →
  installs, colour scheme chosen interactively when several exist; the pager
  closes after one theme operation
- **installed theme** → removal summary → confirm uninstall; deactivates and
  clears `current_theme`/`color_scheme` only if that exact theme is active

Rows already installed show a green `✔ installed` status; enabled snippets
show green keys. Colors respect `NO_COLOR` and disappear when piping.

Piped output and `--json` always print the complete result set without prompts;
`--page N` limits fetching to a single GitHub results page.

### info

```sh
spicepm info Comfy-Themes/Spicetify
spicepm info https://github.com/rxri/spicetify-extensions --json
```

Shows repo metadata plus every valid manifest (id, authors, branch, tags,
download URLs) after the same blacklist check used by install.

### install / uninstall / update

```sh
spicepm install rxri/spicetify-extensions#adblockify
spicepm install Comfy-Themes/Spicetify#Comfy      # scheme prompt if needed
spicepm install https://github.com/someone/some-theme
spicepm uninstall adblock                          # fragment match + y/N
spicepm uninstall adblock --yes                    # skip confirmation
spicepm update StarryNight                         # clean-reinstall one item
spicepm update                                     # everything
```

- **Extensions** land in `<SPICETIFY_CONFIG>/Extensions/<file>` and are
  registered under `[AdditionalOptions] extensions`.
- **Themes** land in `Themes/<name>/` - `user.css`, `color.ini`, every
  `include[]` file, and the first JS include is bridged to `theme.js` so
  spicetify auto-injects it. `current_theme` + your chosen `color_scheme` are
  written to `[Setting]`. If the theme ships scripts, spicepm also sets
  `inject_theme_js=1` for you.
- **Theme updates are clean reinstalls**: the folder is wiped and rebuilt from
  upstream, local drift (edits/orphans) is detected and reported, and your
  previously selected colour scheme is restored when it still exists.
- After mutating commands spicepm prints `run "spicetify apply"`; pass global
  `--apply` to run it for you.

### Lockfile

```sh
spicepm lock                       # write <SPICETIFY_CONFIG>/spicepm/spicepm.lock
spicepm lock --out ~/dotfiles/spicepm.lock
cd ~/dotfiles && spicepm install   # restore everything, schemes included
```

The lockfile records each pinned item (kind, id, user/repo, branch, chosen
colour scheme) plus enabled snippet keys. It is **auto-refreshed on every
install/uninstall/update**, so it never drifts from reality. Zero-arg install
resolves `--lockfile` → `./spicepm.lock` → error with guidance.

### snippets

```sh
spicepm snippets list              # interactive pager, digits toggle
spicepm snippets add "Hamsters Dancing"
spicepm snippets remove "Hamsters Dancing"
spicepm snippets show "Sonic Dancing"
spicepm snippets installed
```

Enabled snippets are applied through a generated companion extension
(`Extensions/spicepm-snippets.js`) that injects them at runtime - the same
mechanism the marketplace app uses, surviving theme switches with zero theme
file pollution. The companion is rebuilt automatically whenever extensions are
installed/uninstalled, and orphaned files in `Extensions/` are cleaned up.

### theme

```sh
spicepm theme set                  # pick from installed themes interactively
spicepm theme set Cattpuccin mocha
spicepm theme scheme latte
spicepm theme current --json
```

### cache

Responses are cached on disk with per-type TTLs (search 10 min, manifests 24 h,
blacklist/snippets 1 h).

```sh
spicepm cache path                 # print the cache directory
spicepm cache clear
```

---

## How it behaves

- **Identity**: every item gets a unique, meaningful id -
  `{user}/{repo}#{Manifest Name}` (e.g. `Comfy-Themes/Spicetify#Comfy`) that
  matches the install target syntax; the snippet companion lives at reserved id
  `@spicepm/snippets`.
- **Ledger**: `<SPICETIFY_CONFIG>/spicepm/ledger.json` tracks provenance
  (source, branch, resolved URLs, sha256 per file, config references). This is
  what powers `update`, exact uninstalls, and `STATUS` marks.
- **Atomicity**: config and ledger writes go through temp-file renames; failed
  actions leave the previous state intact.
- **Rate limits**: exhausted quota produces a clear message with the reset
  time; retries cover transient network/server errors.
- **Safety**: paths recorded in the ledger cannot escape the spicetify dir;
  downloads overwrite atomically; two items can't claim the same extension
  filename.

## Marketplace compliance

`spicepm` ports the marketplace's remote logic field by field:

| [Publishing rule](https://github.com/spicetify/marketplace/wiki/Publishing-to-Marketplace) | spicepm |
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
