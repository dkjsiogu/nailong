# nailong

[![CI](https://github.com/dkjsiogu/nailong/actions/workflows/ci.yml/badge.svg)](https://github.com/dkjsiogu/nailong/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/dkjsiogu/nailong?label=release)](https://github.com/dkjsiogu/nailong/releases/latest)
[![License](https://img.shields.io/badge/license-GPL--3.0--or--later-blue)](LICENSE)

A small, native Minecraft: Java Edition launcher written in Rust. Windows ships as a single executable, Linux as a single `.deb`. No bundled Java runtime, and no installer needed on Windows either.

[简体中文](README.md) | **English**

> Current version: **0.7.2** (preview)
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

### Modpacks

Six formats are detected automatically:

| Format | Entry point | Notes |
|---|---|---|
| Modrinth | `.mrpack` | Direct URLs plus SHA1 |
| CurseForge | zip with `manifest.json` | Resolve project/file IDs; optional API key for hashes |
| Prism / MultiMC | `mmc-pack.json` | Components and overrides |
| MCBBS | `mcbbs.packmeta` | addons + files + overrides |
| HMCL | `modpack.json` + `minecraft/pack.json` | Embedded manifest becomes a profile |
| ATLauncher | `instance.json` | Embedded manifest becomes a profile |

Drag a `.mrpack` or `.zip` into the window to start an import. A single mod `.jar` is not treated as a modpack.

### Everyday use

- Offline-account launch (singleplayer / LAN).
- Version management: base versions required by a loader are hidden from the main picker and protected from deletion.
- Install progress: stage, percentage, downloaded / total bytes, and file count.
- GUI and CLI share the same core; the CLI is useful for scripts and debugging.

## Download

| Platform | File | Size | Requirements |
|---|---|---:|---|
| Windows x64 | [nailong.exe](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong.exe) | ~7.7 MB | Windows 10/11, Java 21 |
| Linux x64 (deb) | [nailong_0.7.2_amd64.deb](https://github.com/dkjsiogu/nailong/releases/latest/download/nailong_0.7.2_amd64.deb) | ~3.2 MB | Ubuntu 24.04+ / glibc ≥ 2.39, Java 21 |

Checksums: [SHA256SUMS](https://github.com/dkjsiogu/nailong/releases/latest/download/SHA256SUMS).

### Windows

1. Download `nailong.exe`, put it anywhere, and double-click it.
2. The first screen is the setup page: choose a Minecraft version, then Fabric / NeoForge / Forge / vanilla, and click **Install**.
3. When the progress finishes, return to the main screen and click **Launch**.

Java 21 must be installed. nailong searches for it automatically and shows a warning if none is found.

> The executable is not code-signed, so SmartScreen may show “Unknown publisher”. Choose **More info → Run anyway**. Antivirus false positives can also happen.

### Linux (Debian / Ubuntu)

```bash
sudo apt install ./nailong_0.7.2_amd64.deb
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

## CurseForge API key (optional)

**Regular users should leave this empty.** It is an application-level API key, not a CurseForge account password, and CurseForge imports work without it.

- With a key: the official API returns the file name, size, SHA1, and download URL, so downloads can be verified.
- Without a key: cfwidget provides the file name and the download goes through CurseForge's official download page; there is no official SHA1.
- If the author has disabled third-party distribution, the import stops and lists the affected files. nailong does not synthesize CDN URLs to bypass that setting.
- Where to enter it: **Settings** in the top-right corner, or the `CURSEFORGE_API_KEY` environment variable.
- Storage: `nailong.json` in the data directory; mode `0600` on Unix, never logged. It is **plaintext on disk**, so do not share the file.
- Priority: runtime environment variable > settings file > build-time default key (maintainers inject `NAILONG_CURSEFORGE_API_KEY`).

## What has been verified

Ran for real:

- Linux (Xvfb + Mesa): Fabric 0.19.5, NeoForge 21.1.209, and Forge 52.1.20 on 1.21.1 reached the main menu.
- Fabulously Optimized `.mrpack`: 151 mods loaded and launched.
- CurseForge: a real file was downloaded through the no-key official-page fallback.
- Prism, MCBBS, HMCL, and ATLauncher: real or synthetic samples passed format detection and instance creation.

Not verified yet:

- Windows is built natively in CI but has not been launched on a real Windows machine.
- Microsoft account login is not implemented; it needs a Minecraft Services client-id allowlist first.
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

## Not implemented yet

- Microsoft account login (waiting for a client-id / Minecraft Services allowlist).
- Auto-update, mod management, and skins.
- An install cancel button (closing the window interrupts; the next launch checks and repairs).
- Real Windows testing and macOS support.
- Importing a single mod `.jar`.

## Project layout

```text
crates/
  nailong-core/   manifests, downloads, Java detection, launch, loaders, modpacks
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
