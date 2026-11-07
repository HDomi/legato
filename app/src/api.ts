// Tauri 백엔드 명령 래퍼.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface HostInfo {
  port: number;
  fingerprint: string;
  short_fingerprint: string;
  display_password: string | null;
  allow_display_password: boolean;
  rotate_display_password: boolean;
  has_access_code: boolean;
  screen_capture: boolean;
  accessibility: boolean;
  addresses: string[];
  config_dir: string;
}

export interface UpdateInfo {
  current: string;
  latest: string;
  available: boolean;
  notes: string;
  url: string;
  has_asset: boolean;
  permissions_reset: boolean;
}

export interface AppSettings {
  auto_host: boolean;
  launch_at_login: boolean;
  lang: 'ko' | 'en';
  auto_update_check: boolean;
  caps_hangul: boolean;
  os: 'macos' | 'windows' | 'other';
}

export interface AppInfo {
  version: string;
  adhoc: boolean;
}

export interface Entry {
  name: string;
  address: string;
  /** 키체인에 접속 코드를 저장했는지 (표시용, 값은 키체인에만 있음). */
  saved_code?: boolean;
}

/** 자식 프로세스 출력 한 줄. kind=event 면 value 는 {event, data} JSON. */
export interface ProcLine {
  tag: string;
  kind: 'event' | 'log' | 'exit';
  value?: any;
}

export const api = {
  hostInfo: () => invoke<HostInfo>('host_info'),
  hostRunning: () => invoke<boolean>('host_running'),
  hostStart: () => invoke<void>('host_start'),
  hostStop: () => invoke<void>('host_stop'),
  hostResetPassword: () => invoke<string>('host_reset_password'),
  hostRequestPermissions: () => invoke<{ screen_capture: boolean; accessibility: boolean }>('host_request_permissions'),
  hostSetAccessCode: (code: string) => invoke<void>('host_set_access_code', { code }),
  hostClearAccessCode: () => invoke<void>('host_clear_access_code'),
  hostConfigure: (opts: { rotate?: boolean; allowDisplay?: boolean }) => invoke<void>('host_configure', opts),
  connect: (opts: {
    id: string;
    address: string;
    password: string;
    useSaved: boolean;
    remember: boolean;
    windowed: boolean;
    acceptNewFingerprint: boolean;
  }) => invoke<void>('connect', opts),
  forgetCode: (address: string) => invoke<void>('forget_code', { address }),
  serviceStatus: () => invoke<{ installed: boolean; running: boolean; stale?: boolean; supported?: boolean }>('service_status'),
  serviceChange: (install: boolean) => invoke<void>('service_change', { install }),
  loginAgentStatus: () => invoke<{ installed: boolean; stale?: boolean; supported: boolean }>('login_agent_status'),
  loginAgentChange: (install: boolean) => invoke<void>('login_agent_change', { install }),
  checkUpdate: () => invoke<UpdateInfo>('check_update'),
  installUpdate: (version: string, force: boolean) => invoke<void>('install_update', { version, force }),
  getSettings: () => invoke<AppSettings>('get_settings'),
  setSettings: (
    s: Partial<{ autoHost: boolean; launchAtLogin: boolean; lang: string; autoUpdateCheck: boolean; capsHangul: boolean }>,
  ) => invoke<void>('set_settings', s),
  appInfo: () => invoke<AppInfo>('app_info'),
  onCodeSaved: (cb: (p: { tag: string; address: string; error: string | null }) => void): Promise<UnlistenFn> =>
    listen<{ tag: string; address: string; error: string | null }>('code-saved', (e) => cb(e.payload)),
  loadAddresses: () => invoke<Entry[]>('load_addresses'),
  saveAddresses: (entries: Entry[]) => invoke<void>('save_addresses', { entries }),
  onHost: (cb: (l: ProcLine) => void): Promise<UnlistenFn> => listen<ProcLine>('host', (e) => cb(e.payload)),
  onViewer: (cb: (l: ProcLine) => void): Promise<UnlistenFn> => listen<ProcLine>('viewer', (e) => cb(e.payload)),
};
