#!/usr/bin/env bash
# Build FSearchFFI.xcframework (device, simulator, macOS) and the Swift bindings.
# Run on macOS with Xcode and rustup. The CLI release profile is left alone;
# this uses the thinner release-ffi profile so five target builds stay practical.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PROFILE=release-ffi
KIT="$ROOT/apple/FSearchKit"
STAGE="$ROOT/target/xcframework-stage"
HEADERS="$STAGE/headers"

if ! command -v xcodebuild >/dev/null; then
  echo "xcodebuild is required (run this on macOS)" >&2
  exit 1
fi

rustup target add \
  aarch64-apple-ios \
  aarch64-apple-ios-sim \
  x86_64-apple-ios \
  aarch64-apple-darwin \
  x86_64-apple-darwin

build_target() {
  echo "==> $1"
  cargo build --profile "$PROFILE" -p fsearch-ffi --target "$1"
}

build_target aarch64-apple-ios
build_target aarch64-apple-ios-sim
build_target x86_64-apple-ios
build_target aarch64-apple-darwin
build_target x86_64-apple-darwin

lib_for() {
  echo "$ROOT/target/$1/$PROFILE/libfsearch_ffi.a"
}

# Bindgen loads the dylib, so it has to match this machine.
case "$(uname -m)" in
  arm64) HOST_ARCH=aarch64-apple-darwin ;;
  x86_64) HOST_ARCH=x86_64-apple-darwin ;;
  *) echo "unsupported host arch $(uname -m)" >&2; exit 1 ;;
esac
HOST_LIB="$ROOT/target/$HOST_ARCH/$PROFILE/libfsearch_ffi.dylib"
if [[ ! -f "$HOST_LIB" ]]; then
  echo "no host dylib at $HOST_LIB" >&2
  exit 1
fi

BINDINGS="$KIT/Sources/FSearchFFIBindings"
mkdir -p "$BINDINGS" "$KIT/bindings"
cargo run -p fsearch-ffi --features cli --bin uniffi-bindgen -- \
  generate --library "$HOST_LIB" --language swift --out-dir "$KIT/bindings"
# The Swift file is compiled into the package. The header ships inside the xcframework.
mv -f "$KIT/bindings/FSearchFFI.swift" "$BINDINGS/FSearchFFI.swift"

rm -rf "$STAGE"
mkdir -p "$HEADERS" "$STAGE/ios-sim" "$STAGE/macos"
cp "$KIT/bindings/FSearchFFI.h" "$HEADERS/FSearchFFI.h"
cat > "$HEADERS/module.modulemap" <<'EOF'
module FSearchFFI {
    header "FSearchFFI.h"
    export *
}
EOF

lipo -create \
  "$(lib_for aarch64-apple-ios-sim)" \
  "$(lib_for x86_64-apple-ios)" \
  -output "$STAGE/ios-sim/libfsearch_ffi.a"

lipo -create \
  "$(lib_for aarch64-apple-darwin)" \
  "$(lib_for x86_64-apple-darwin)" \
  -output "$STAGE/macos/libfsearch_ffi.a"

rm -rf "$KIT/FSearchFFI.xcframework"
xcodebuild -create-xcframework \
  -library "$(lib_for aarch64-apple-ios)" -headers "$HEADERS" \
  -library "$STAGE/ios-sim/libfsearch_ffi.a" -headers "$HEADERS" \
  -library "$STAGE/macos/libfsearch_ffi.a" -headers "$HEADERS" \
  -output "$KIT/FSearchFFI.xcframework"

# create-xcframework leaves the module map beside the headers. SwiftPM loads
# Modules/module.modulemap and resolves its header next to that file.
while IFS= read -r slice; do
  if [[ -f "$slice/Headers/module.modulemap" && -f "$slice/Headers/FSearchFFI.h" ]]; then
    mkdir -p "$slice/Modules"
    cp "$slice/Headers/FSearchFFI.h" "$slice/Modules/FSearchFFI.h"
    cp "$slice/Headers/module.modulemap" "$slice/Modules/module.modulemap"
  fi
done < <(find "$KIT/FSearchFFI.xcframework" -mindepth 1 -maxdepth 1 -type d)

echo "wrote $KIT/FSearchFFI.xcframework"
