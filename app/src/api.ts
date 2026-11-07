// Tauri 백엔드 명령 래퍼.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export interface HostInfo {
  port: number;
  fingerprint: string;
  short_fingerprint: string;
  has_password: boolean;
  screen_capture: boolean;
  accessibility: boolean;
  addresses: string[];
  config_dir: string;
}

export interface Entry {
  name: string;
  address: string;
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
  connect: (id: string, address: string, password: string, windowed: boolean, acceptNewFingerprint: boolean) =>
    invoke<void>('connect', { id, address, password, windowed, acceptNewFingerprint }),
  loadAddresses: () => invoke<Entry[]>('load_addresses'),
  saveAddresses: (entries: Entry[]) => invoke<void>('save_addresses', { entries }),
  onHost: (cb: (l: ProcLine) => void): Promise<UnlistenFn> => listen<ProcLine>('host', (e) => cb(e.payload)),
  onViewer: (cb: (l: ProcLine) => void): Promise<UnlistenFn> => listen<ProcLine>('viewer', (e) => cb(e.payload)),
};
