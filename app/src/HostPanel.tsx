import { useCallback, useEffect, useState } from 'react';
import { api, type HostInfo } from './api';

/** 이 컴퓨터를 원격으로 열어주는 쪽 (호스트). */
export default function HostPanel() {
  const [info, setInfo] = useState<HostInfo | null>(null);
  const [running, setRunning] = useState(false);
  const [password, setPassword] = useState<string | null>(null);
  const [sessions, setSessions] = useState<string[]>([]);
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setInfo(await api.hostInfo());
      setRunning(await api.hostRunning());
      setError(null);
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
    // 시스템 설정에서 권한을 바꾸고 돌아오면 상태를 다시 읽는다.
    window.addEventListener('focus', refresh);
    const un = api.onHost((l) => {
      if (l.kind === 'exit') {
        if (l.tag === 'stdout') {
          setRunning(false);
          setSessions([]);
        }
        return;
      }
      if (l.kind === 'log') {
        setLogs((prev) => [...prev.slice(-199), String(l.value)]);
        return;
      }
      const { event, data } = l.value;
      if (event === 'listening') setRunning(true);
      if (event === 'password_generated') setPassword(data.password);
      if (event === 'session_started') setSessions((s) => [...s, `${data.client} (${data.peer})`]);
      if (event === 'session_ended') setSessions((s) => s.filter((x) => !x.endsWith(`(${data.peer})`)));
    });
    return () => {
      window.removeEventListener('focus', refresh);
      un.then((f) => f());
    };
  }, [refresh]);

  const toggle = async () => {
    setError(null);
    try {
      if (running) {
        await api.hostStop();
        setRunning(false);
        setSessions([]);
      } else {
        await api.hostStart();
      }
    } catch (e) {
      setError(String(e));
    }
  };

  const resetPassword = async () => {
    try {
      const pw = await api.hostResetPassword();
      setPassword(pw);
      // 실행 중이면 새 비밀번호를 적용하려고 다시 시작한다.
      if (running) {
        await api.hostStop();
        await api.hostStart();
      }
      refresh();
    } catch (e) {
      setError(String(e));
    }
  };

  const requestPermissions = async () => {
    try {
      await api.hostRequestPermissions();
    } finally {
      refresh();
    }
  };

  const needsPermission = info && (!info.screen_capture || !info.accessibility);

  return (
    <section className="panel">
      <div className="card hero">
        <div>
          <h2>{running ? '접속 대기 중' : '꺼져 있음'}</h2>
          <p className="muted">
            {running
              ? '아래 주소와 비밀번호로 다른 컴퓨터에서 접속할 수 있어.'
              : '켜면 이 컴퓨터 화면을 원격으로 열어줘.'}
          </p>
        </div>
        <button className={running ? 'danger' : 'primary'} onClick={toggle}>
          {running ? '끄기' : '켜기'}
        </button>
      </div>

      {error && <div className="card error">{error}</div>}

      {needsPermission && (
        <div className="card warn">
          <strong>권한이 필요해</strong>
          <ul>
            {!info!.screen_capture && <li>화면 기록: 화면을 보내려면 필요해</li>}
            {!info!.accessibility && <li>손쉬운 사용: 원격 마우스·키보드 조작에 필요해</li>}
          </ul>
          <p className="muted">macOS 는 이 권한을 원격으로 허용할 수 없어서, 처음 한 번은 이 컴퓨터 앞에서 직접 허용해야 해.</p>
          <button onClick={requestPermissions}>권한 요청</button>
        </div>
      )}

      {info && (
        <div className="card grid">
          <div className="label">주소</div>
          <div className="mono">
            {info.addresses.length ? info.addresses.map((a) => `${a}:${info.port}`).join('  ·  ') : '네트워크 없음'}
          </div>
          <div className="label">비밀번호</div>
          <div className="row">
            <span className="mono">{password ?? (info.has_password ? '설정됨 (보이지 않음)' : '없음')}</span>
            <button className="small" onClick={resetPassword}>
              새로 만들기
            </button>
          </div>
          <div className="label">지문</div>
          <div className="mono small-text">{info.short_fingerprint}</div>
          <div className="label">접속 중</div>
          <div>{sessions.length ? sessions.join(', ') : '없음'}</div>
        </div>
      )}

      <p className="muted small-text">
        외부 네트워크에서 접속하려면 공유기에서 UDP {info?.port ?? 47820} 포트포워딩이 필요해. 가능하면 Tailscale·WireGuard 같은 VPN
        안에서만 쓰는 걸 추천해.
      </p>

      {logs.length > 0 && (
        <details className="card">
          <summary>로그</summary>
          <pre className="log">{logs.join('\n')}</pre>
        </details>
      )}
    </section>
  );
}
