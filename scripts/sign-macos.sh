#!/usr/bin/env bash
# Legato.app 을 자체 서명 인증서로 서명한다.
#
# Apple 의 codesign 은 신뢰되지 않은(자체 서명) 인증서를 "identity 없음" 으로 거부하므로,
# p12 를 직접 읽어 서명하는 rcodesign(apple-codesign) 을 쓴다. 시스템 신뢰 설정을 바꿀 필요가 없다.
# 결과 지정 요구사항은 `certificate root = H"<인증서 SHA-1>"` 이라 같은 인증서로 서명하면
# 업데이트 사이에도 macOS 권한(화면 기록, 손쉬운 사용)이 유지된다.
#
# 사용법: scripts/sign-macos.sh <Legato.app 경로>
# 환경 변수:
#   MACOS_CERT_P12_BASE64, MACOS_CERT_PASSWORD - make-signing-cert.sh 결과 (없으면 ad-hoc 서명 + 경고)
set -euo pipefail

APP="$1"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ENTITLEMENTS="$ROOT/app/src-tauri/entitlements.plist"
RCODESIGN_VERSION="0.29.0"
RCODESIGN_SHA256="d98372d5524226ccf9dc0eda03d4e4f5826182dabb2fc3f2bd303ed9113a748d"

if [ -z "${MACOS_CERT_P12_BASE64:-}" ]; then
  echo "::warning::MACOS_CERT_P12_BASE64 가 없어서 ad-hoc 서명해. 업데이트할 때마다 macOS 권한이 초기화돼." >&2
  codesign --force --deep --sign - "$APP"
  codesign --verify --deep --strict "$APP"
  exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# rcodesign 이 PATH 에 없으면 고정 버전을 받아 체크섬을 확인한다.
RCODESIGN="$(command -v rcodesign || true)"
if [ -z "$RCODESIGN" ]; then
  TAR="apple-codesign-$RCODESIGN_VERSION-macos-universal.tar.gz"
  curl -fsSL -o "$WORK/$TAR" \
    "https://github.com/indygreg/apple-platform-rs/releases/download/apple-codesign/$RCODESIGN_VERSION/$TAR"
  echo "$RCODESIGN_SHA256  $WORK/$TAR" | shasum -a 256 -c - >/dev/null
  tar -xzf "$WORK/$TAR" -C "$WORK"
  RCODESIGN="$WORK/apple-codesign-$RCODESIGN_VERSION-macos-universal/rcodesign"
fi

printf '%s' "$MACOS_CERT_P12_BASE64" | base64 --decode > "$WORK/cert.p12"
printf '%s' "$MACOS_CERT_PASSWORD" > "$WORK/pass.txt"

# 번들 안 실행 파일(도우미 포함)까지 안쪽부터 자동으로 서명한다. 하드닝된 런타임 사용.
"$RCODESIGN" sign \
  --p12-file "$WORK/cert.p12" \
  --p12-password-file "$WORK/pass.txt" \
  --code-signature-flags runtime \
  --entitlements-xml-file "$ENTITLEMENTS" \
  "$APP"

codesign --verify --strict --verbose=2 "$APP"
echo "지정 요구사항 (업데이트 사이에 같아야 권한이 유지됨):"
codesign -d -r- "$APP" 2>&1 | grep designated
