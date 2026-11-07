#!/usr/bin/env bash
# Mac 릴리스 산출물을 만든다. GitHub Actions(release.yml)와 로컬에서 똑같이 쓴다.
#   테스트 → clippy → 앱 빌드(사이드카 포함) → 서명 → DMG · zip
# 결과: out/Legato-<버전>-arm64.dmg, out/Legato-<버전>-arm64-mac.zip
#
# 사용법: scripts/build-mac-release.sh
# 서명 인증서: MACOS_CERT_P12_BASE64, MACOS_CERT_PASSWORD (없으면 ad-hoc 서명 + 경고)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

step() { printf '\n==> %s\n' "$*"; }

step "Xcode · SDK 확인"
xcodebuild -version
SDK="$(xcrun --sdk macosx --show-sdk-version)"
echo "macOS SDK $SDK"
# ScreenCaptureKit 바인딩(apple-metal)이 macOS 26 SDK API 를 쓴다. 낮으면 Swift 컴파일에서 실패하므로 미리 막는다.
if [ "${SDK%%.*}" -lt 26 ]; then
  echo "::error::macOS SDK $SDK 는 너무 낮아. Xcode 26 이상이 필요해 (sudo xcode-select -s /Applications/Xcode_26*.app)" >&2
  exit 1
fi

VERSION="$(node -p 'require("./app/src-tauri/tauri.conf.json").version')"
echo "버전 $VERSION"

step "테스트"
cargo test --locked

step "clippy"
cargo clippy --locked -- -D warnings

step "앱 빌드"
(cd app && npm ci --no-audit --no-fund && npx tauri build --bundles app)
APP="target/release/bundle/macos/Legato.app"

step "서명"
scripts/sign-macos.sh "$APP"

step "DMG · zip"
rm -rf out dmgroot
mkdir -p out dmgroot
cp -R "$APP" dmgroot/
ln -s /Applications dmgroot/Applications
hdiutil create -volname "Legato" -srcfolder dmgroot -ov -format UDZO "out/Legato-$VERSION-arm64.dmg"
rm -rf dmgroot
# 앱 안 업데이터용 zip (서명 · 확장 속성을 보존하려고 ditto 사용).
ditto -c -k --keepParent "$APP" "out/Legato-$VERSION-arm64-mac.zip"

step "결과"
ls -lh out/
