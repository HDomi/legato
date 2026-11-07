// 화면 문구 사전 (한국어 · 영어). 한국어는 존댓말로 쓴다. 기본 언어는 한국어.
// 언어는 설정 탭에서 바꾸고 app.json 에 저장된다 (호스트 · 뷰어에도 같은 언어가 넘어간다).
import { createContext, useCallback, useContext, useMemo, type ReactNode } from 'react';

export type Lang = 'ko' | 'en';

const ko = {
  'tab.connect': '접속하기',
  'tab.host': '이 컴퓨터 열기',
  'tab.settings': '설정',

  // 접속하기
  'connect.address': '주소',
  'connect.addressPlaceholder': '192.168.0.10 또는 192.168.0.10:47820',
  'connect.secret': '비밀번호 또는 접속 코드',
  'connect.secretPlaceholder': '표시 비밀번호나 고정 접속 코드',
  'connect.remember.mac': '고정 접속 코드면 키체인에 저장 (다음부터 바로 접속)',
  'connect.remember.win': '고정 접속 코드면 자격 증명 관리자에 저장 (다음부터 바로 접속)',
  'connect.windowed': '전체화면 Space 대신 창으로 열기',
  'connect.go': '접속',
  'connect.namePlaceholder': '이름 (주소록 저장용)',
  'connect.addToBook': '주소록에 추가',
  'connect.book': '주소록',
  'connect.saved': '코드 저장됨',
  'connect.quick': '바로 접속',
  'connect.forget': '코드 지우기',
  'connect.delete': '삭제',
  'connect.sessions': '세션',
  'connect.connectedTo': '{host}에 연결되었습니다',
  'connect.viaCode': ' (접속 코드)',
  'connect.ended': '연결이 끝났습니다',
  'connect.notSaved': '저장하지 않았습니다: {reason}',
  'connect.trustNew': '새 지문 믿고 다시 접속',
  'connect.typeCodeToTrust': '호스트 지문이 바뀌었습니다. 호스트에 직접 확인한 뒤 접속 코드를 입력하고 다시 접속해 주세요.',
  'connect.hostLocked': '호스트 화면이 잠겨 있습니다. 원격 창에서 비밀번호를 입력해 잠금을 해제해 주세요.',

  // 이 컴퓨터 열기
  'host.waiting': '접속 대기 중',
  'host.off': '꺼져 있음',
  'host.waitingDesc': '아래 주소로 다른 컴퓨터에서 접속할 수 있습니다.',
  'host.offDesc': '켜면 이 컴퓨터 화면을 원격으로 열어 드립니다.',
  'host.serviceWaiting': '서비스 모드로 대기 중',
  'host.turnOn': '켜기',
  'host.turnOff': '끄기',
  'host.permTitle': '권한이 필요합니다',
  'host.permScreen': '화면 기록: 화면을 보내려면 필요합니다',
  'host.permAx': '손쉬운 사용: 원격으로 마우스 · 키보드를 조작하려면 필요합니다',
  'host.permDesc': 'macOS 는 이 권한을 원격으로 허용할 수 없어서, 처음 한 번은 이 컴퓨터 앞에서 직접 허용해 주셔야 합니다.',
  'host.permRequest': '권한 요청',
  'host.address': '주소',
  'host.noNetwork': '네트워크 없음',
  'host.fingerprint': '지문',
  'host.connected': '접속 중',
  'host.none': '없음',
  'host.viaCode': ' · 접속 코드',
  'host.displayPw': '표시 비밀번호',
  'host.displayPwDesc': '옆 사람에게 알려 주어도 되는 임시 비밀번호입니다. 켤 때마다 바뀌게 할 수 있습니다.',
  'host.displayPwPending': '켜면 만들어집니다',
  'host.regenerate': '새로 만들기',
  'host.displayPwOff': '꺼져 있습니다. 고정 접속 코드로만 접속할 수 있습니다.',
  'host.rotate': '켤 때마다 새로 만들기',
  'host.allowDisplay': '표시 비밀번호로 접속 허용',
  'host.allowDisplayNeedsCode': '(끄려면 고정 접속 코드를 먼저 정해 주세요)',
  'host.code': '고정 접속 코드',
  'host.codeDesc':
    '나만 아는 비밀입니다. 이 컴퓨터에는 해시만 저장되어 다시 볼 수 없고, 직접 바꾸기 전에는 바뀌지 않습니다. 다른 컴퓨터에서 이 코드로 접속하며 저장해 두면 다음부터 클릭 한 번으로 들어올 수 있습니다.',
  'host.codeSet': '설정됨',
  'host.change': '바꾸기',
  'host.set': '설정',
  'host.clear': '지우기',
  'host.newCode': '새 접속 코드 (10자 이상)',
  'host.again': '한 번 더 입력',
  'host.save': '저장',
  'host.cancel': '취소',
  'host.codeTooShort': '고정 접속 코드는 10자 이상이어야 합니다.',
  'host.codeMismatch': '두 번 입력한 코드가 다릅니다.',
  'host.codeSaved': '고정 접속 코드를 저장했습니다. 이 화면에는 다시 보이지 않으니 따로 기억해 두세요.',
  'host.codeClearBlocked': '표시 비밀번호가 꺼져 있어 접속 코드를 지우면 아무도 접속할 수 없습니다. 표시 비밀번호를 먼저 켜 주세요.',
  'host.codeCleared': '고정 접속 코드를 지웠습니다.',
  'host.service': 'Windows 서비스 모드',
  'host.serviceDesc':
    'Windows 가 켜지면 로그인 전부터 접속을 받습니다. 로그인 화면 · 잠금 화면 · UAC(관리자 허용) 창도 원격으로 보고 누를 수 있고, 원격 메뉴의 Ctrl+Alt+Del 도 동작합니다. 설치와 제거에는 관리자 권한이 필요합니다.',
  'host.serviceRunning': '실행 중',
  'host.serviceStopped': '설치됨 (멈춤)',
  'host.serviceOff': '꺼짐',
  'host.serviceInstall': '서비스 설치',
  'host.serviceRemove': '서비스 제거',
  'host.working': '처리 중…',
  'host.serviceOn': '서비스 모드를 켰습니다. 로그인 화면 · UAC 창까지 원격으로 다룰 수 있습니다.',
  'host.serviceOffDone': '서비스 모드를 껐습니다.',
  'host.loginAgent': '로그인 화면에서도 접속 (macOS)',
  'host.loginAgentDesc':
    '로그아웃했거나 재부팅한 뒤 로그인 화면에서도 접속을 받습니다 (고정 접속 코드 필요). 관리자 암호가 필요합니다. FileVault 를 켠 Mac 은 재부팅 직후 디스크 잠금 해제 화면에서는 접속할 수 없습니다 (macOS 가 뜨기 전 단계이기 때문입니다).',
  'host.on': '켜짐',
  'host.offState': '꺼짐',
  'host.needCodeFirst': '고정 접속 코드를 먼저 정해 주세요',
  'host.loginAgentOn': '로그인 화면 접속을 켰습니다. 로그아웃한 상태에서도 고정 접속 코드로 들어올 수 있습니다.',
  'host.loginAgentOffDone': '로그인 화면 접속을 껐습니다.',
  'host.loginAgentStale': '앱이 업데이트되었거나 접속 코드가 바뀌었습니다. 로그인 화면 접속에도 반영하려면 다시 적용해 주세요.',
  'host.loginAgentReapply': '다시 적용',
  'host.serviceStale': '앱이 업데이트되었습니다. 서비스도 새 버전을 쓰도록 다시 적용해 주세요.',
  'host.network':
    '외부 네트워크에서 접속하려면 공유기에서 UDP {port} 포트포워딩이 필요합니다. 가능하면 Tailscale · WireGuard 같은 VPN 안에서만 사용하시길 권장합니다.',
  'host.logs': '로그',
  'host.locked': '이 컴퓨터 화면이 잠겨 있습니다',

  // 업데이트
  'update.badge': 'v{version} 업데이트',
  'update.latest': '최신 버전입니다',
  'update.checkFailed': '업데이트 확인 실패: {error}',
  'update.noNotes': '릴리스 노트를 가져오지 못했습니다. 릴리스 페이지에서 확인할 수 있습니다: {url}',
  'update.permReset':
    '지금 앱은 임시(ad-hoc) 서명이라 업데이트하면 화면 기록 · 손쉬운 사용 권한을 이 컴퓨터 앞에서 다시 허용해야 합니다. 원격으로만 사용 중이라면 업데이트 뒤 접속이 안 될 수 있습니다.',
  'update.noAsset': '이 기기용 설치 파일이 릴리스에 없습니다. 릴리스 페이지에서 직접 받아 주세요.',
  'update.install': '업데이트하고 다시 열기',
  'update.downloading': '받는 중…',
  'update.later': '나중에',
  'update.busyConfirm': '{reason}\n업데이트하면 연결이 끊어집니다. 그래도 진행할까요?',

  // 설정
  'settings.language': '언어',
  'settings.languageDesc': '앱 화면과 원격 창(뷰어)의 언어입니다.',
  'settings.update': '업데이트',
  'settings.version': '현재 버전',
  'settings.checkNow': '지금 확인',
  'settings.checking': '확인 중…',
  'settings.available': '새 버전 v{version} 이 있습니다',
  'settings.upToDate': '최신 버전입니다',
  'settings.autoCheck': '앱을 켤 때와 6시간마다 자동으로 확인',
  'settings.signing': '서명 · 권한 유지',
  'settings.signedStable':
    '고정 인증서로 서명된 앱입니다. 업데이트해도 화면 기록 · 손쉬운 사용 권한과 저장한 접속 코드가 그대로 유지됩니다.',
  'settings.signedAdhoc':
    '임시(ad-hoc) 서명 앱입니다. 업데이트하면 macOS 권한을 다시 허용해야 합니다. 고정 인증서로 서명한 릴리스부터는 유지됩니다.',
  'settings.keepData': '고정 접속 코드 · 주소록 · 설정은 앱 밖(사용자 설정 폴더)에 있어서 업데이트해도 지워지지 않습니다.',
  'settings.keychainNote':
    '저장한 접속 코드는 로그인 키체인에 보관되며, 업데이트 뒤에도 키체인 암호를 다시 묻지 않도록 이 사용자 계정의 다른 앱도 읽을 수 있게 저장됩니다. 공용 컴퓨터에서는 코드를 저장하지 마세요.',
  'settings.startup': '시작',
  'settings.launchAtLogin': '로그인하면 Legato 실행',
  'settings.autoHost': 'Legato 가 켜지면 바로 접속 대기',
  'settings.startupDesc': '밖에서 접속하려면 이 컴퓨터가 켜져서 기다리고 있어야 합니다. 대기 중에는 잠자기를 막습니다 (화면은 꺼져도 됩니다).',
  'settings.input': '입력',
  'settings.capsHangul': 'Caps Lock 을 원격 한/영 전환 키로 사용',
  'settings.capsHangulDesc': '원격 컴퓨터의 한/영 입력을 바꿉니다. 뷰어 메뉴에서도 바꿀 수 있습니다.',
} as const;

export type Key = keyof typeof ko;

const en: Record<Key, string> = {
  'tab.connect': 'Connect',
  'tab.host': 'Share this computer',
  'tab.settings': 'Settings',

  'connect.address': 'Address',
  'connect.addressPlaceholder': '192.168.0.10 or 192.168.0.10:47820',
  'connect.secret': 'Password or access code',
  'connect.secretPlaceholder': 'Display password or access code',
  'connect.remember.mac': 'Save access code to Keychain (one-click next time)',
  'connect.remember.win': 'Save access code to Credential Manager (one-click next time)',
  'connect.windowed': 'Open in a window instead of a full-screen Space',
  'connect.go': 'Connect',
  'connect.namePlaceholder': 'Name (for the address book)',
  'connect.addToBook': 'Add to address book',
  'connect.book': 'Address book',
  'connect.saved': 'Code saved',
  'connect.quick': 'Connect now',
  'connect.forget': 'Forget code',
  'connect.delete': 'Delete',
  'connect.sessions': 'Sessions',
  'connect.connectedTo': 'Connected to {host}',
  'connect.viaCode': ' (access code)',
  'connect.ended': 'Connection ended',
  'connect.notSaved': 'Not saved: {reason}',
  'connect.trustNew': 'Trust the new fingerprint and reconnect',
  'connect.typeCodeToTrust': 'The host fingerprint changed. Check it on the host, then type the access code and reconnect.',
  'connect.hostLocked': 'The host screen is locked. Type the password in the remote window to unlock it.',

  'host.waiting': 'Waiting for connections',
  'host.off': 'Off',
  'host.waitingDesc': 'Other computers can connect using the address below.',
  'host.offDesc': 'Turn on to let others view and control this computer.',
  'host.serviceWaiting': 'Waiting in service mode',
  'host.turnOn': 'Turn on',
  'host.turnOff': 'Turn off',
  'host.permTitle': 'Permissions needed',
  'host.permScreen': 'Screen Recording: needed to send the screen',
  'host.permAx': 'Accessibility: needed for remote mouse and keyboard',
  'host.permDesc': "macOS doesn't allow granting these remotely, so the first time you need to allow them at this computer.",
  'host.permRequest': 'Request permissions',
  'host.address': 'Address',
  'host.noNetwork': 'No network',
  'host.fingerprint': 'Fingerprint',
  'host.connected': 'Connected',
  'host.none': 'None',
  'host.viaCode': ' · access code',
  'host.displayPw': 'Display password',
  'host.displayPwDesc': 'A temporary password you can share with someone nearby. It can change every time the host starts.',
  'host.displayPwPending': 'Created when turned on',
  'host.regenerate': 'Regenerate',
  'host.displayPwOff': 'Off. Only the access code can be used.',
  'host.rotate': 'Regenerate on every start',
  'host.allowDisplay': 'Allow connecting with the display password',
  'host.allowDisplayNeedsCode': '(set an access code first to turn this off)',
  'host.code': 'Access code',
  'host.codeDesc':
    "A secret only you know. Only a hash is stored here, so it can't be shown again, and it never changes unless you change it. Save it when connecting from another computer and you can connect with one click next time.",
  'host.codeSet': 'Set',
  'host.change': 'Change',
  'host.set': 'Set',
  'host.clear': 'Clear',
  'host.newCode': 'New access code (10+ characters)',
  'host.again': 'Enter it again',
  'host.save': 'Save',
  'host.cancel': 'Cancel',
  'host.codeTooShort': 'The access code must be at least 10 characters.',
  'host.codeMismatch': "The two codes don't match.",
  'host.codeSaved': "Access code saved. It won't be shown again, so keep it somewhere safe.",
  'host.codeClearBlocked':
    'The display password is off, so clearing the access code would lock everyone out. Turn the display password on first.',
  'host.codeCleared': 'Access code cleared.',
  'host.service': 'Windows service mode',
  'host.serviceDesc':
    'Accepts connections from boot, before anyone signs in. The sign-in screen, lock screen and UAC prompts can be seen and used remotely, and Ctrl+Alt+Del works from the remote menu. Installing and removing need administrator rights.',
  'host.serviceRunning': 'Running',
  'host.serviceStopped': 'Installed (stopped)',
  'host.serviceOff': 'Off',
  'host.serviceInstall': 'Install service',
  'host.serviceRemove': 'Remove service',
  'host.working': 'Working…',
  'host.serviceOn': 'Service mode is on. The sign-in screen and UAC prompts can now be used remotely.',
  'host.serviceOffDone': 'Service mode is off.',
  'host.loginAgent': 'Accept connections at the login window (macOS)',
  'host.loginAgentDesc':
    "Accepts connections at the login window after logging out or restarting (access code required). Needs your administrator password. Macs with FileVault can't be reached at the disk unlock screen right after a restart (that's before macOS starts).",
  'host.on': 'On',
  'host.offState': 'Off',
  'host.needCodeFirst': 'Set an access code first',
  'host.loginAgentOn': 'Login-window access is on. You can connect with the access code even when logged out.',
  'host.loginAgentOffDone': 'Login-window access is off.',
  'host.loginAgentStale': 'The app was updated or the access code changed. Reapply to use them at the login window too.',
  'host.loginAgentReapply': 'Reapply',
  'host.serviceStale': 'The app was updated. Reapply so the service uses the new version too.',
  'host.network':
    'To connect from outside your network, forward UDP {port} on your router. Using it only inside a VPN such as Tailscale or WireGuard is recommended.',
  'host.logs': 'Logs',
  'host.locked': 'This computer is locked',

  'update.badge': 'Update to v{version}',
  'update.latest': 'Up to date',
  'update.checkFailed': 'Update check failed: {error}',
  'update.noNotes': "Couldn't load the release notes. See the release page: {url}",
  'update.permReset':
    "This app is ad-hoc signed, so after updating you'll need to allow Screen Recording and Accessibility again at this computer. If you only use it remotely, you may not be able to connect after the update.",
  'update.noAsset': 'The release has no installer for this device. Please download it from the release page.',
  'update.install': 'Update and relaunch',
  'update.downloading': 'Downloading…',
  'update.later': 'Later',
  'update.busyConfirm': '{reason}\nUpdating will disconnect them. Continue?',

  'settings.language': 'Language',
  'settings.languageDesc': 'Language of the app and the remote (viewer) windows.',
  'settings.update': 'Updates',
  'settings.version': 'Current version',
  'settings.checkNow': 'Check now',
  'settings.checking': 'Checking…',
  'settings.available': 'Version v{version} is available',
  'settings.upToDate': 'Up to date',
  'settings.autoCheck': 'Check automatically at launch and every 6 hours',
  'settings.signing': 'Signing and permissions',
  'settings.signedStable':
    'Signed with a fixed certificate. Screen Recording / Accessibility permissions and saved access codes are kept across updates.',
  'settings.signedAdhoc':
    "Ad-hoc signed. After updating you'll need to allow the macOS permissions again. Releases signed with the fixed certificate keep them.",
  'settings.keepData': 'Your access code, address book and settings live outside the app (in your settings folder), so updates never delete them.',
  'settings.keychainNote':
    "Saved access codes are kept in your login keychain. So that updates don't ask for the keychain password again, they are stored so other apps running as this user can read them. Don't save codes on shared computers.",
  'settings.startup': 'Startup',
  'settings.launchAtLogin': 'Open Legato at login',
  'settings.autoHost': 'Start waiting for connections when Legato opens',
  'settings.startupDesc':
    "To connect from outside, this computer must be on and waiting. While waiting it won't go to sleep (the display may turn off).",
  'settings.input': 'Input',
  'settings.capsHangul': 'Use Caps Lock as the remote Korean/English key',
  'settings.capsHangulDesc': "Switches the remote computer's Korean/English input. Also available in the viewer menu.",
};

const dicts: Record<Lang, Record<Key, string>> = { ko, en };

type Ctx = { lang: Lang; setLang: (l: Lang) => void };
export const LangContext = createContext<Ctx>({ lang: 'ko', setLang: () => {} });

export function LangProvider({ lang, setLang, children }: Ctx & { children: ReactNode }) {
  const value = useMemo(() => ({ lang, setLang }), [lang, setLang]);
  return <LangContext.Provider value={value}>{children}</LangContext.Provider>;
}

/** `t('host.network', { port: 47820 })` 처럼 쓴다. */
export function useI18n() {
  const { lang, setLang } = useContext(LangContext);
  const t = useCallback(
    (key: Key, vars?: Record<string, string | number>) => {
      let s: string = dicts[lang][key] ?? ko[key];
      if (vars) for (const [k, v] of Object.entries(vars)) s = s.split(`{${k}}`).join(String(v));
      return s;
    },
    [lang],
  );
  return { t, lang, setLang };
}
