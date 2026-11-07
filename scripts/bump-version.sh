#!/usr/bin/env bash
# 버전을 한 번에 올린다: Cargo workspace, tauri.conf.json, app/package.json.
# 사용법: scripts/bump-version.sh 0.2.0
# 그다음: git commit -am "release: v0.2.0" && git tag -a v0.2.0 -m "..." && git push --follow-tags
set -euo pipefail
V="${1:?버전을 넣어줘 (예: 0.2.0)}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
[[ "$V" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "형식은 X.Y.Z" >&2; exit 1; }

# workspace.package 의 version 한 줄만 바꾼다.
perl -0pi -e "s/(\[workspace\.package\]\nversion = )\"[^\"]+\"/\$1\"$V\"/" "$ROOT/Cargo.toml"
# node -e 에서는 argv[1] 부터가 추가 인자다.
V="$V" node -e '
const fs = require("fs");
for (const f of process.argv.slice(1)) {
  const j = JSON.parse(fs.readFileSync(f, "utf8"));
  j.version = process.env.V;
  fs.writeFileSync(f, JSON.stringify(j, null, 2) + "\n");
}' "$ROOT/app/src-tauri/tauri.conf.json" "$ROOT/app/package.json"
(cd "$ROOT/app" && npm install --package-lock-only --no-audit --no-fund >/dev/null)
(cd "$ROOT" && cargo update --workspace >/dev/null 2>&1 || true)
echo "버전 $V 로 바꿨어"
