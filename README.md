# spicepm

A Spicetify **Marketplace package manager** for your terminal — discover, install,
update, and remove themes, extensions, and CSS snippets using the **same sources,
fetching logic, and filtering rules as the official
[Spicetify Marketplace](https://github.com/spicetify/marketplace)**.

Built in Rust (edition 2024). Linux-first, also runs on macOS and Windows.

## Why

The official marketplace is a Spotify-embedded app. `spicepm` gives the same
catalogue to the CLI workflow: scriptable installs, reproducible setup, no UI
required.

## Install / build

```sh
cargo install --path .        # or: cargo build --release
```

Requires [spicetify](https://spicetify.app) to be installed (`SPICETIFY_CONFIG`
is honoured if you use a custom location).

### GitHub API token (recommended)

Unauthenticated GitHub API calls are limited to 60/hour. Export a token to raise
the limit:

```fish
set -x GITHUB_TOKEN ghp_xxxx    # fish
export GITHUB_TOKEN=ghp_xxxx   # bash/zsh
```

`GH_TOKEN` and `SPICEPM_GITHUB_TOKEN` are also accepted.

## Commands

```
spicepm search [query] [--type ext|theme] [--sort stars|newest|oldest|lastUpdated|mostStale|a-z|z-a]
spicepm info <user/repo|url>              # manifests, stars, authors, tags
spicepm install <user/repo[#name]|url>    # extension or theme
spicepm uninstall <id|name|file>
spicepm update [item]                     # default: everything
spicepm list installed
spicepm snippets list|show|add|remove|installed
spicepm theme set <folder> [scheme] | scheme <scheme> | current
spicepm cache path|clear
```

Global flags: `--no-cache`, `--apply` (run `spicetify apply` automatically),
`-v/-vv` logging, `--json` on read commands.

On a TTY, `search` pages results 10 at a time — navigate with `←`/`→`
(or `p`/`n`), jump with `g`/`G`, quit with `q`, and press a row's number
(`0`–`9`) to act on it: uninstalled extensions install (and stay browsing),
installed extensions toggle to uninstall, themes show a full install
summary and ask for confirmation. Every selection spells out exactly which
files land where before anything happens. Piped output and `--json`
always print the full result set; `--page N` limits fetching to a single
GitHub results page.

### Examples

```sh
spicepm search bloom --type theme --sort stars
spicepm install Comfy-Themes/Spicetify#Comfy
spicepm snippets add "Hamsters Dancing"
spicepm theme set Comfy mocha
spicepm update all --apply
```

## Marketplace compliance

`spicepm` ports the marketplace's remote logic (`FetchRemotes.ts`) exactly:

| [Publishing rule](https://github.com/spicetify/marketplace/wiki/Publishing-to-Marketplace) | spicepm |
|---|---|
| Discovery via `spicetify-extensions` / `spicetify-themes` topics | ✅ topic search, `per_page=100`, paginated |
| `manifest.json` in repo root | ✅ fetched from raw + default branch |
| Array manifests (multi-extension repos) | ✅ every entry expanded individually |
| Field requirements/optionality | ✅ mirrors the app's zod schema field-by-field (more lenient than the wiki templates where the app is too) |
| `branch` override / default branch fallback | ✅ |
| Authors fallback to repo owner; author URLs | ✅ incl. dangerous-scheme neutralization |
| http(s) URL support for `preview/main/readme/usercss/schemes` | ✅ verbatim when absolute, raw-relative otherwise |
| Blacklist + archived-repo filtering | ✅ glob semantics identical (`*` = one path segment) |
| Theme `schemes` → colour-scheme choice | ✅ parsed & prompted at install |
| Theme `include[]` scripts | ✅ downloaded **and** bridged to `theme.js` so spicetify auto-injects them on disk |
| Snippets from `resources/snippets.json` | ✅ applied via companion extension |
| Custom apps (`spicetify-apps`) | ⏳ planned milestone |
| Search results per manifest item | ✅ multi-extension repos list each entry separately; manifest-less repos hidden |

Additional details:

## How installs work

- **Extensions** are downloaded to `<SPICETIFY_CONFIG>/Extensions/` and
  registered in `config-xpui.ini` under `[AdditionalOptions] extensions`
  (pipe-separated list, deduplicated).
- **Themes** download `user.css` → `Themes/<name>/user.css`,
  schemes → `color.ini`, and every `include[]` file (relative layout preserved;
  absolute-URL entries store under their filename). You pick a colour scheme at
  install time; `current_theme` / `color_scheme` are set in `[Setting]`.
- **Snippets** are applied through a generated companion extension
  (`Extensions/spicepm-snippets.js`) that injects the enabled CSS at runtime —
  the same mechanism the marketplace app uses, surviving theme switches without
  touching theme files.

Everything installed is tracked in `<SPICETIFY_CONFIG>/spicepm/ledger.json`
(provenance + file hashes), which powers `update` and clean uninstalls.
Config edits are atomic; responses are cached on disk with TTLs
(`cache clear` resets).

## Development

```sh
cargo test          # 47 unit tests (schema parity, blacklist globs, INI roundtrip, ...)
cargo clippy --all-targets   # zero warnings, pedantic lints on
cargo fmt --check
```

Roadmap: custom apps support (`topic:spicetify-apps`, M6), shell completions +
man page, `self-update` via marketplace releases API, wiremock-based
integration suite, cargo-dist release pipeline.

## License

MIT
