#!/bin/bash
# Build ProdMesh Director.app and ProdMesh Playout.app.
#
#   tools/package-macos.sh              both, unsigned, into dist-macos/
#   tools/package-macos.sh --sign       and Developer ID sign them
#   tools/package-macos.sh playout      just the one
#
# Two apps, one launcher binary. What differs between them is a JSON file in
# Resources — see ADR 0029. Signing and notarization are separate steps on
# purpose: an unsigned build is what you want fifty times a day, and a notarized
# one is what you want at a release.
set -euo pipefail
cd "$(dirname "$0")/.."
REPO=$PWD
OUT=$REPO/dist-macos
VERSION=$(node -p "require('./package.json').version")
SIGN=0
WANT=${*:-}
for a in "$@"; do [ "$a" = "--sign" ] && SIGN=1; done

say() { printf "\n\033[1m== %s\033[0m\n" "$*"; }

want() {
  case "$WANT" in
    *playout*) [ "$1" = playout ] ;;
    *director*) [ "$1" = director ] ;;
    *) true ;;
  esac
}

say "1. the pieces"
cargo build --release -p pmg-node
npm run build >/dev/null

# The launcher, once. Both bundles get the same binary — two launchers with two
# codebases would drift, and only one of them would be the one that mattered.
say "2. the launcher"
mkdir -p "$OUT"
swiftc -O -target arm64-apple-macos13 -o "$OUT/ProdMeshLauncher" macos/launcher/*.swift

bundle() {
  local kind=$1 name=$2 id=$3
  local app="$OUT/$name.app"
  say "3. $name.app"
  rm -rf "$app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

  cp "$OUT/ProdMeshLauncher" "$app/Contents/MacOS/ProdMeshLauncher"
  sed -e "s|@NAME@|$name|g" -e "s|@BUNDLE_ID@|$id|g" \
      -e "s|@EXECUTABLE@|ProdMeshLauncher|g" -e "s|@VERSION@|$VERSION|g" \
      macos/Info.plist.in > "$app/Contents/Info.plist"
  cp "macos/$kind/Launcher.json" "$app/Contents/Resources/Launcher.json"

  # The splash, if there is one. Optional on purpose: a missing piece of art
  # must not stop a booth machine being built.
  if [ -f "assets/splash-$kind.png" ]; then
    cp "assets/splash-$kind.png" "$app/Contents/Resources/Splash.png"
  else
    echo "   (no assets/splash-$kind.png — the app just won't show one)"
  fi
  [ -f assets/AppIcon.icns ] && cp assets/AppIcon.icns "$app/Contents/Resources/"

  case "$kind" in
    playout)
      cp target/release/pmg-node "$app/Contents/Resources/pmg-node"
      ;;
    director)
      # The Node runtime travels WITH the app. A booth machine has no nvm and
      # should never be asked to install one; an app that depends on whatever
      # `node` happens to be on the PATH is an app that breaks when somebody
      # upgrades something unrelated.
      cp "$(command -v node)" "$app/Contents/Resources/node"
      mkdir -p "$app/Contents/Resources/server" "$app/Contents/Resources/dist"
      cp -R server/. "$app/Contents/Resources/server/"
      rm -f "$app/Contents/Resources/server/"*.test.js
      cp -R dist/. "$app/Contents/Resources/dist/"
      # Production dependencies only — the dev half is vite, vitest and friends,
      # which are 200 MB of things a booth machine will never run.
      cp package.json "$app/Contents/Resources/"
      (cd "$app/Contents/Resources" && npm install --omit=dev --ignore-scripts --silent >/dev/null 2>&1 || true)
      # better-sqlite3 ships a prebuilt binary but the install above skipped
      # scripts, so take the one that already works in this repo.
      if [ -d node_modules/better-sqlite3 ]; then
        rm -rf "$app/Contents/Resources/node_modules/better-sqlite3"
        cp -R node_modules/better-sqlite3 "$app/Contents/Resources/node_modules/"
      fi
      ;;
  esac

  if [ "$SIGN" = 1 ]; then
    local ident="${PMG_SIGN_IDENTITY:-Developer ID Application}"
    # Inside out: a bundle is signed from its deepest nested code outwards, or
    # the outer signature is invalidated by the inner ones.
    find "$app/Contents/Resources" -type f \( -perm -u+x -o -name "*.node" -o -name "*.dylib" \) \
      -exec codesign --force --timestamp --options runtime \
      --entitlements macos/entitlements.plist --sign "$ident" {} \; 2>/dev/null || true
    codesign --force --timestamp --options runtime \
      --entitlements macos/entitlements.plist --sign "$ident" "$app"
    codesign --verify --deep --strict --verbose=2 "$app"
  fi

  echo "   $(du -sh "$app" | cut -f1)  $app"
}

want playout  && bundle playout  "ProdMesh Playout"  com.prodmesh.graphics.playout
want director && bundle director "ProdMesh Director" com.prodmesh.graphics.director
rm -f "$OUT/ProdMeshLauncher"

say "done"
cat <<EOF

  Both apps live in the menu bar — no Dock icon, no window. They keep their
  data in ~/Library/Application Support, so replacing an app is dragging a new
  one in and nothing is lost.

    open "$OUT/ProdMesh Director.app"
    open "$OUT/ProdMesh Playout.app"

  Playout finds Director on the network by itself. Give it an address by hand
  by editing Launcher.json in the bundle, or start it from a terminal with
  --server ws://host:8519.

  Unsigned, so the first launch needs a right-click > Open. --sign fixes that
  and is what a release does.
EOF
