#!/usr/bin/env bash
# Build a Debian package for the current machine architecture.
# Usage: scripts/build-deb.sh
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$root"

cargo_bin=${CARGO:-$(command -v cargo || true)}
if [ -z "$cargo_bin" ]; then cargo_bin="$HOME/.cargo/bin/cargo"; fi

version=$(awk -F'"' '/^version = /{print $2; exit}' Cargo.toml)
arch=$(dpkg --print-architecture)
package="nailong"
out_dir="$root/dist"
stage=$(mktemp -d)
trap 'rm -rf "$stage"' EXIT

echo "==> building GUI binary (release)"
"$cargo_bin" build --release -p nailong-ui

bin="$root/target/release/nailong-ui"
glibc_req=$(objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sed 's/^GLIBC_//' | sort -V | tail -1)
if [ -z "$glibc_req" ]; then
  echo "could not determine glibc requirement" >&2
  exit 1
fi

install -Dm755 "$bin" "$stage/usr/bin/nailong"
install -Dm644 packaging/linux/io.github.dkjsiogu.nailong.desktop \
  "$stage/usr/share/applications/io.github.dkjsiogu.nailong.desktop"
install -Dm644 packaging/linux/io.github.dkjsiogu.nailong.metainfo.xml \
  "$stage/usr/share/metainfo/io.github.dkjsiogu.nailong.metainfo.xml"
install -Dm644 packaging/linux/io.github.dkjsiogu.nailong.svg \
  "$stage/usr/share/icons/hicolor/scalable/apps/io.github.dkjsiogu.nailong.svg"
install -Dm644 packaging/linux/copyright "$stage/usr/share/doc/nailong/copyright"
install -Dm644 README.md "$stage/usr/share/doc/nailong/README.md"
install -Dm644 LICENSE "$stage/usr/share/doc/nailong/LICENSE"

for size in 16 24 32 48 64 128 256 512; do
  icon_dir="$stage/usr/share/icons/hicolor/${size}x${size}/apps"
  mkdir -p "$icon_dir"
  convert -background none packaging/linux/io.github.dkjsiogu.nailong.svg \
    -resize "${size}x${size}" -depth 8 "$icon_dir/io.github.dkjsiogu.nailong.png"
done

cat > "$stage/usr/share/doc/nailong/changelog.Debian" <<EOF
nailong ($version) unstable; urgency=medium

  * Initial public preview.

 -- dkjsiogu <123802475+dkjsiogu@users.noreply.github.com>  $(date -R)
EOF
gzip -9n "$stage/usr/share/doc/nailong/changelog.Debian"

chmod 755 "$stage"
mkdir -p "$stage/DEBIAN"
installed_kb=$(du -sk --exclude=DEBIAN "$stage" | cut -f1)

cat > "$stage/DEBIAN/control" <<EOF
Package: $package
Version: $version
Section: games
Priority: optional
Architecture: $arch
Installed-Size: $installed_kb
Maintainer: dkjsiogu <123802475+dkjsiogu@users.noreply.github.com>
Depends: libc6 (>= $glibc_req), libgcc-s1, libgl1, libegl1, libx11-6, libxext6, libxcursor1, libxi6, libxrandr2, libxkbcommon0
Recommends: openjdk-21-jre | default-jre
Homepage: https://github.com/dkjsiogu/nailong
Description: Lightweight Minecraft launcher (NeoForge preview)
 nailong is a lightweight Minecraft: Java Edition launcher written in Rust.
 .
 This preview installs Minecraft 1.21.1 and NeoForge 21.1.209, then launches
 the game with an offline account. It needs a Java 21 runtime installed.
EOF

(cd "$stage" && find . -type f ! -path './DEBIAN/*' -printf '%P\n' | sort | xargs md5sum > DEBIAN/md5sums)

mkdir -p "$out_dir"
deb="$out_dir/${package}_${version}_${arch}.deb"
dpkg-deb --root-owner-group --build "$stage" "$deb" >/dev/null

echo
echo "built: $deb"
dpkg-deb --info "$deb" | sed -n '1,22p'
