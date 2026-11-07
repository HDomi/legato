# Legato

윈도우 <-> 맥 화면공유를 편하게 하기위한 앱

Mac ↔ Mac, Windows ↔ Windows, Mac ↔ Windows 원격 화면·조작. 원격받는 쪽이 UDP 포트 하나(기본 `47820`)만 열어두면 IP 로 바로 접속한다. 중계 서버 없음.

- **Mac 뷰어**: 원격 화면이 네이티브 전체화면(별도 Space)으로 열린다. Mission Control 에서 Space 로 오간다.
- **Windows 뷰어**: 일반 창.
- **보안**: QUIC(TLS 1.3) 암호화, 호스트 인증서 지문 고정(처음 접속 때 기억, 바뀌면 거부), 비밀번호는 네트워크로 보내지 않는 상호 챌린지 인증(PBKDF2 + HMAC + TLS 채널 바인딩), IP 별 5회 실패 시 잠금.

> 현재 단계: **P1 (Mac 호스트 + Mac/Windows 뷰어 MVP)**. Windows 호스트, 멀티 모니터, UAC·로그인 화면은 다음 단계. 아래 [로드맵](#로드맵) 참고.

## 구조

```
crates/legato-core    프로토콜, QUIC 전송, 인증, H.264 코덱(OpenH264)
crates/legato-host    원격받는 쪽. 화면 캡처(ScreenCaptureKit) → 인코딩 → 전송, 입력 주입(CGEvent)
crates/legato-viewer  접속하는 쪽. 디코딩 → wgpu 렌더, 입력 전송. 세션마다 별도 프로세스
app/                  Tauri v2 + React 앱. 주소록·호스트 켜기·접속 UI, 위 두 실행 파일을 띄운다
scripts/              사이드카 빌드, 서명, 버전, 아이콘
```

## 쓰기

### 앱

1. [릴리스](https://github.com/HDomi/legato/releases/latest) 에서 Mac 은 `Legato-<버전>-arm64.dmg`, Windows 는 `Legato_<버전>_x64-setup.exe` 를 받는다.
2. **이 컴퓨터 열기** 탭에서 켜기. 표시되는 주소와 비밀번호를 접속할 쪽에 알려준다.
3. 다른 컴퓨터의 **접속하기** 탭에 주소·비밀번호를 넣고 접속.

macOS 호스트는 **화면 기록**과 **손쉬운 사용** 권한이 필요하다. Apple 정책상 원격으로 허용할 수 없어서 처음 한 번은 그 Mac 앞에서 직접 허용해야 한다. macOS 15 이상은 "개인정보 보호 윈도우 선택기 우회" 확인 창이 주기적으로 뜰 수 있다.

### 명령줄

```bash
# 호스트
legato-host set-password --generate   # 비밀번호 새로 만들기 (원문은 저장 안 함)
legato-host serve                     # 대기 (--port, --max-width 1920, --fps 30, --bitrate 8000000)
legato-host info                      # 주소, 지문, 권한 상태
legato-host request-permissions       # macOS 권한 요청 창 띄우기

# 뷰어 (비밀번호는 LEGATO_PASSWORD 환경 변수 또는 프롬프트)
legato-viewer 192.168.0.10            # Mac 은 전체화면 Space, --windowed 로 창 모드
legato-viewer 192.168.0.10 --headless --frames 120 --dump frame.png   # 창 없이 FPS·지연 측정
```

뷰어 창 단축키: Mac 에서 `⌘Q` 는 뷰어 종료(원격으로 전달되지 않음), `⌃⌘F` 는 전체화면 전환.

### 외부에서 접속

공유기에서 UDP `47820` 포트포워딩이 필요하다. **원격 제어 포트를 인터넷에 그대로 여는 건 위험하다.** 가능하면 Tailscale·WireGuard 같은 VPN 안에서만 쓴다.

## 개발

필요: Rust stable, Node 22, (Mac) Xcode.

```bash
cargo test                       # core·host·viewer 테스트
cargo build --release            # target/release/legato-host, legato-viewer

cd app && npm install
npm run sidecars                 # 호스트·뷰어를 릴리스로 빌드해 Tauri 사이드카 위치로 복사 (dev 전에 한 번)
npx tauri dev                    # 앱 개발 모드
npx tauri build --bundles app    # target/release/bundle/macos/Legato.app
```

설정 위치: Mac `~/Library/Application Support/com.hdomi.Legato/`, Windows `%APPDATA%\hdomi\Legato\config\`. `LEGATO_CONFIG_DIR` 로 바꿀 수 있다 (여러 인스턴스 테스트용).

## 릴리스

Vibrato 와 같은 방식: 버전 태그를 푸시하면 GitHub Actions 가 빌드해서 릴리스에 올린다.

```bash
scripts/bump-version.sh 0.2.0
git commit -am "release: v0.2.0"
git tag -a v0.2.0 -m "변경 내용"
git push --follow-tags
```

결과물: `Legato-<v>-arm64.dmg`, `Legato-<v>-arm64-mac.zip`(앱 안 업데이터용), `Legato_<v>_x64-setup.exe`, `SHA256SUMS.txt`.

### Mac 서명 (자체 서명 인증서)

ad-hoc 서명은 빌드마다 서명 해시가 바뀌어 macOS 가 화면 기록·손쉬운 사용 권한을 업데이트마다 초기화한다. 원격 앱에는 치명적이라 **같은 자체 서명 인증서로 계속 서명**한다. 그러면 지정 요구사항이 `certificate root = H"…"` 로 고정돼 권한이 유지된다.

1. 한 번만: `scripts/make-signing-cert.sh ~/legato-signing` (레포 밖에 만든다)
2. GitHub 레포 Secrets 에 `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD` 등록
3. 그 폴더는 안전한 곳에 백업. 잃어버리면 새 인증서로 바뀌어 모든 Mac 에서 권한을 한 번 다시 허용해야 한다.

서명은 `scripts/sign-macos.sh` 가 [rcodesign](https://github.com/indygreg/apple-platform-rs) 으로 한다 (Apple `codesign` 은 신뢰되지 않은 자체 서명 인증서를 거부해서). Secrets 가 없으면 ad-hoc 서명으로 대신하고 경고를 남긴다. 공증(notarization)은 하지 않으므로 처음 실행 때 "그래도 열기" 가 필요하다.

## 로드맵

| 단계 | 내용 |
|---|---|
| P0 ✅ | workspace, CI, 릴리스 workflow, 자체 서명 |
| P1 ✅ | Mac 호스트, Mac/Windows 뷰어, 암호화·상호 인증, Tauri 앱 |
| P2 | Windows 호스트 (Windows.Graphics.Capture, SendInput) |
| P3 | 멀티 모니터(모니터별 Space), Cmd↔Ctrl 매핑, 클립보드, 하드웨어 코덱, 화질 자동 조절, 뷰어 툴바 |
| P4 | Windows SYSTEM 서비스: UAC·잠금·로그인 화면, Ctrl+Alt+Del |
| P5 | macOS LaunchDaemon: 로그인 화면, 관리자 암호 창 검증 |
| P6 | 앱 안 업데이터 |
