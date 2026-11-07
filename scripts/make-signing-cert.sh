#!/usr/bin/env bash
# macOS 코드 서명용 자체 서명 인증서를 한 번 만든다.
#
# 왜 필요한가: ad-hoc 서명은 빌드마다 서명 해시가 바뀌어서 macOS 가 "화면 기록"·"손쉬운 사용" 권한을
# 업데이트마다 초기화한다. 같은 인증서로 계속 서명하면 지정 요구사항(designated requirement)이 고정돼
# 권한이 유지된다.
#
# 사용법: scripts/make-signing-cert.sh [출력 폴더]
# 결과물: legato-signing.p12 (개인키 포함, 절대 커밋 금지), 그리고 GitHub Secrets 에 넣을 값 안내.
set -euo pipefail

OUT="${1:-./signing}"
NAME="Legato Self-Signed"
mkdir -p "$OUT"
chmod 700 "$OUT"
PASS="$(openssl rand -base64 24)"

cat > "$OUT/cert.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
O = HDomi
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF

# 10년짜리 인증서. 만료되면 새로 만들어야 하고, 그때는 권한을 한 번 다시 허용해야 한다.
openssl req -x509 -newkey rsa:3072 -sha256 -days 3650 -nodes \
  -keyout "$OUT/key.pem" -out "$OUT/cert.pem" -config "$OUT/cert.cnf" 2>/dev/null
# macOS security 가 읽을 수 있게 레거시 암호화로 묶는다.
openssl pkcs12 -export -legacy -inkey "$OUT/key.pem" -in "$OUT/cert.pem" -name "$NAME" \
  -out "$OUT/legato-signing.p12" -passout "pass:$PASS" 2>/dev/null \
  || openssl pkcs12 -export -inkey "$OUT/key.pem" -in "$OUT/cert.pem" -name "$NAME" \
       -out "$OUT/legato-signing.p12" -passout "pass:$PASS"
rm -f "$OUT/key.pem" "$OUT/cert.cnf"
chmod 600 "$OUT/legato-signing.p12"

base64 < "$OUT/legato-signing.p12" | tr -d '\n' > "$OUT/MACOS_CERT_P12_BASE64.txt"
printf '%s' "$PASS" > "$OUT/MACOS_CERT_PASSWORD.txt"
chmod 600 "$OUT"/*.txt

cat <<MSG
만들었어: $OUT/legato-signing.p12
GitHub 레포 Settings > Secrets and variables > Actions 에 두 개를 넣어줘:
  MACOS_CERT_P12_BASE64 = $OUT/MACOS_CERT_P12_BASE64.txt 내용
  MACOS_CERT_PASSWORD   = $OUT/MACOS_CERT_PASSWORD.txt 내용
넣은 뒤에는 $OUT 폴더를 안전한 곳(비밀번호 관리자 등)에 백업하고 레포 밖에 보관해.
MSG
