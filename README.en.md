# nailong

[![CI](https://github.com/dkjsiogu/nailong/actions/workflows/ci.yml/badge.svg)](https://github.com/dkjsiogu/nailong/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/dkjsiogu/nailong?label=release)](https://github.com/dkjsiogu/nailong/releases/latest)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

A small, native Minecraft: Java Edition launcher written in Rust. Windows ships as a single executable, Linux as a single `.deb`. No bundled Java runtime, and no installer needed on Windows either.

[简体中文](README.md) | **English**

> Current version: **0.16.0** (preview)
>
> Goals: small binaries, few dependencies, visible install progress, and a repair path after interruption. See [implementation status](docs/design/14-implementation-status.md) for exact boundaries.

## Why nailong

Installing Minecraft is not the hard part — doing it cleanly is. Version JSON, asset indexes, loader processors, JVM flags, and a few hundred files per modpack: one mistake anywhere and the player just sees “launch failed”.

nailong tries to do one thing simply:

- Written in Rust; the launcher itself does not run on a JVM. One file on Windows, one package on Linux.
- Installation is split into visible stages; if interrupted, the next run checks what is missing and resumes instead of restarting.
- Forge/NeoForge client processors run directly — the official installer is never invoked or left on disk.
- Drop in a modpack and nailong detects the format, installs the loader, downloads files, and applies overrides.

It is not a universal launcher, and it is not 1.0 yet. What works and what does not is laid out below.

## Interface

0.16 rebuilds the UI around a [design system](docs/design/15-ui-design-system.md): dark neutral surfaces, one warm accent, a fixed page header, and a single scrolling content area. Each page has one primary action; deletes confirm inline; empty states point at the next step.

| Home | Download center |
|---|---|
| ![Home](docs/screenshots/01-main.png) | ![Download center](docs/screenshots/04-browse.png) |
| **Project page** | **Content manager** |
| ![Project page](docs/screenshots/06-detail.png) | ![Content manager](docs/screenshots/07-installed.png) |

More screenshots (version manager, install, search, updates, saves, progress, account dialogs): [docs/screenshots](docs/screenshots).

## Features

### Install and launch

- Vanilla, Fabric, NeoForge, Forge — versions come from the official manifests, no manual entry.
- Native Forge/NeoForge install: we fetch only `install_profile`, `version.json`, and the client patch (~3 MB). The full installer is never kept.
- Once the version JSON is ready, assets, client/libraries, loader patches, and processors run in parallel where possible.
- Large files download in four ranged segments with resumable parts and automatic fallback to a mirror source.
- A machine-wide loader-patch cache; client/libraries use SHA1-addressed blobs and are hardlinked into place. Existing files only get a cheap size check.
- Patched clients found locally are reused: we scan official / PCL / HMCL / Prism / MultiMC / Modrinth and sibling `.minecraft` roots; a matching SHA1 (or a valid NeoForge JAR shape) skips the patch and processors.
- Reinstalling an intact Forge/NeoForge instance is a fast no-op — no re-download, no repeated processors.
- Missing assets show up as a specific “repair resources” action instead of a generic launch failure.
- Installs can be cancelled: downloads, extraction, and processors respond; completed parts stay on disk for the next repair/retry.

### Modpacks

Six formats are detected automatically:

| Format | Entry point | Notes |
|---|---|---|
| Modrinth | `.mrpack` | Direct URLs plus SHA1 |
| CurseForge | zip with `manifest.json` | Resolve project/file IDs; official builds include a key for SHA1 checks |
| Prism / MultiMC | `mmc-pack.json` | Components and overrides |
| MCBBS | `mcbbs.packmeta` | addons + files + overrides |
| HMCL | `modpack.json` + `minecraft/pack.json` | Embedded manifest becomes a profile |
| ATLauncher | `instance.json` | Embedded manifest becomes a profile |

Drag a `.mrpack` or `.zip` into the window to start an import. A single mod `.jar` is not treated as a modpack.

### Download center: mods, resource packs and shaders

- Pick “install to: <installed version>” in the download center first. Browsing and search then lock to that version's Minecraft version and loader, and `+` writes straight into its `mods/`, `resourcepacks/` or `shaderpacks/`.
- Prefer window-shopping? Choose “browse only”, pick any version/loader, and `+` will ask for an install target first.
- The download center opens on a browse page: title, search, content type and “install to” stay pinned at the top; the content area is a row of category chips (optimization, technology, adventure, realistic, fantasy, …) plus “Popular” and “Newest” shelves. No keyword required.
- Each card shows the icon, name, summary and download count, with a `+` in its bottom-right corner. It installs the latest build compatible with the selected version and loader, plus required dependencies, and then turns into “installed”.
- Clicking the rest of a card opens a two-column project page: project/version info and dependencies on the left, the install panel and version history on the right. Pick a file version and review `required` / `optional` / `incompatible` dependencies; `required` is added automatically, `optional` is off by default, `incompatible` blocks the download.
- Each shelf has “More ›” in its corner: it opens the full list for that shelf (24 per page) with “load more” pagination.
- Category chips come from **CurseForge's real category tree**: the launcher fetches `/v1/categories` at runtime and translates it, keeping CurseForge-only categories under their own names and appending Modrinth-only ones after them.
- Version and loader are optional: “all versions · all loaders” is the default, so window-shopping works without installing anything first.
- Cards show a short description (name, summary, downloads); the detail page has the full summary plus author and download count.
- The chip row shows 12 categories by default and folds the rest behind “more categories”. A source switch (All / Modrinth / CurseForge) lets you browse the CurseForge catalogue as-is, with its categories and sort orders. Downloads are still resolved and verified by the launcher; no website is involved.
- Search stays in the top bar (Enter to run) and renders the same cards with pagination.
- Files go into that version's `mods/`, `resourcepacks/` or `shaderpacks/`. A normal version gets an isolated `gameDir` (`versions/<id>/`) on first download; existing saves in the root are not migrated.
- The installed page is a real content manager: filter by name, toggle files, check for updates, update one or all, and see duplicate warnings. Delete asks inline, and updating a disabled file keeps it disabled.
- Local `.jar` drag/import is supported, as is local `.zip` import for resource packs and shaders.
- Identical existing files are skipped; conflicts are reported, never silently overwritten. Updates replace the old version and keep the index in sync.

### Everyday use

- Microsoft account login from the top-right button: browser authorization code + PKCE on a loopback port, automatic token refresh, and `--accessToken` / `--uuid` / `--xuid` at launch.
- Tokens live in `account.json` under the data directory (mode 0600 on Unix) and never appear in logs; logging out deletes it.
- Offline-account launch (singleplayer / LAN) still works when nobody is signed in.
- Version management: base versions required by a loader are hidden from the main picker and protected from deletion; every version has “content” and “saves” entries.
- Saves: list worlds with size and modified time. Backups are **git-style snapshots**: content-defined chunking stores only changed chunks, so a second backup usually adds a few MB; restore, delete and prune by count.
- Automatic backups: every 15/30/60/120 minutes, keeping 5/10/20/50 snapshots per world; settings live in `backups.json` under the data directory and run while the launcher is open.
- Resource packs and shaders get the same enable/disable, delete, file-size and open-folder affordances as mods.
- Install progress: stage, percentage, downloaded / total bytes, and file count.
- GUI and CLI share the same core; the CLI is useful for scripts and debugging.

## Download

| Platform | File | Size | Requirements |
|---|---|---:|---|
| Windows x64 | [nailong.exe](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong.exe) | ~8 MB | Windows 10/11, Java 21 |
| Linux x64 (deb) | [nailong_0.16.0_amd64.deb](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong_0.16.0_amd64.deb) | ~3.3 MB | Ubuntu 24.04+ / glibc ≥ 2.39, Java 21 |

Checksums: [SHA256SUMS](https://github.com/dkjsiogu/nailong/releases/latest/download/SHA256SUMS).

### Windows

1. Download `nailong.exe`, put it anywhere, and double-click it.
2. The first screen is the setup page: choose a Minecraft version, then Fabric / NeoForge / Forge / vanilla, and click **Install**.
3. When the progress finishes, return to the main screen and click **Launch**.

Java 21 must be installed. nailong searches for it automatically and shows a warning if none is found.

> The executable is not code-signed, so SmartScreen may show “Unknown publisher”. Choose **More info → Run anyway**. Antivirus false positives can also happen.

### Linux (Debian / Ubuntu)

```bash
sudo apt install ./nailong_0.10.0_amd64.deb
nailong
```

You can also double-click the deb in a software center. After installation, **nailong** appears in the application menu.

Uninstall:

```bash
sudo apt remove nailong
```

> The deb is built on Ubuntu 26.04 / amd64 and requires glibc ≥ 2.39. Ubuntu 24.04+ works; older systems such as 22.04 cannot install it yet.

### Data directory

- Windows: `.minecraft` next to `nailong.exe` (portable).
- Linux: `~/.local/share/nailong/.minecraft`.

Use `--root` to point at another directory.

## CurseForge API (built in)

Official releases include a project key, so CurseForge packs use the official API automatically and **regular users do not configure anything**. It is an application-level key, not a CurseForge account password.

- The official API returns the file name, size, SHA1, and download URL, so downloads can be verified.
- If the author has disabled third-party distribution, the import stops and lists the affected files. nailong does not synthesize CDN URLs to bypass that setting.
- Maintainers can override the built-in key with the `CURSEFORGE_API_KEY` environment variable, so rotating it does not require a code change.
- The key is lightly XOR-obfuscated so a casual `strings` scan does not reveal it. **This is not encryption**: it can still be recovered from the binary and is only a shared default quota, not a secret.
- A local build without an injected key still imports packs through the no-key cfwidget + official-page fallback, without official SHA1.

## What has been verified

Ran for real:

- GUI: every page was walked at 1120×720 and the 900×620 minimum, including browse/search/detail, content manager, update banner, delete confirmation, empty states, save snapshots, progress, and the account/login dialogs; screenshots live in `docs/screenshots/`.
- Linux (Xvfb + Mesa): Fabric 0.19.5, NeoForge 21.1.209, and Forge 52.1.20 on 1.21.1 reached the main menu.
- Fabulously Optimized `.mrpack`: 151 mods loaded and launched.
- CurseForge: a real file was downloaded through the no-key official-page fallback; the approved API key returned the official file name, size, SHA1, and download URL.
- GUI cancellation: cancelled an install under Xvfb and returned to the “continue install” page without crashing.
- Download center: real Modrinth + CurseForge APIs; mod/resource-pack/shader search; Jade plus its required dependency downloaded into the version-isolated `mods/`; installed scan, toggles, duplicate detection and update checks. 0.10.0 adds a real click-through test: the `+` button on a browse card downloaded Fabric API into the isolated `mods/` and wrote the content index.
- Prism, MCBBS, HMCL, and ATLauncher: real or synthetic samples passed format detection and instance creation.

Not verified yet:

- Windows is built natively in CI but has not been launched on a real Windows machine.
- macOS is not supported.

## Command line

The deb only contains the GUI; building from source also produces the CLI:

```bash
./target/release/nailong --root /path/to/.minecraft list
./target/release/nailong --root /path/to/.minecraft install neoforge 1.21.1 21.1.209
./target/release/nailong --root /path/to/.minecraft install modpack ~/Downloads/pack.mrpack
./target/release/nailong --root /path/to/.minecraft install modpack ~/Downloads/pack.zip
./target/release/nailong --root /path/to/.minecraft launch neoforge-21.1.209 --username Steve --memory 4096
```

## Build from source

Rust 1.95+ is required. On Linux, egui also needs system development packages:

```bash
sudo apt install build-essential pkg-config \
  libgl1-mesa-dev libegl1-mesa-dev \
  libx11-dev libxext-dev libxcursor-dev libxi-dev libxrandr-dev \
  libxkbcommon-dev libxkbcommon-x11-dev
```

Build the GUI and CLI:

```bash
cargo build --release -p nailong-ui -p nailong-cli
```

Build the Linux deb:

```bash
./scripts/build-deb.sh
# output: dist/nailong_<version>_<arch>.deb
```

Windows release builds use MSVC with the static CRT in GitHub Actions. `x86_64-pc-windows-gnu` can also be used for a local cross-compile check.

> Building from source without `NAILONG_CURSEFORGE_API_KEY` produces a binary with the **CurseForge source disabled** (Modrinth is unaffected). This is intentional: the key is obfuscated into the binary at build time and never read from a config file.
>
> ```bash
> NAILONG_CURSEFORGE_API_KEY=... cargo build --release -p nailong-ui
> ```
>
> Live network smoke tests (ignored by default):
>
> ```bash
> cargo test -p nailong-core --test live_modrinth -- --ignored --nocapture
> ```

## Not implemented yet

- Microsoft tokens are stored in a 0600 file instead of the OS keychain (Credential Manager / Secret Service).
- Launcher self-update, skins, and full modpack updates.
- Stopping a running game / cancelling a launch.
- Real Windows testing and macOS support.

## Project layout

```text
crates/
  nailong-core/   manifests, downloads, Java detection, launch, loaders, modpacks, content center
  nailong-cli/    command line
  nailong-ui/     egui desktop UI
packaging/linux/  desktop file, icons, AppStream metadata, copyright
scripts/          deb packaging
docs/
  design/         architecture and design notes
  brief/          9-page project brief
```

See the [design index](docs/design/00-overview.md) and the [implementation status](docs/design/14-implementation-status.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).

This project is not affiliated with Mojang Studios or Microsoft, and it does not bundle or redistribute Minecraft game assets.
