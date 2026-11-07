import { useCallback, useEffect, useState } from 'react';
import { api, type AppSettings, type HostInfo } from './api';

/** 이 컴퓨터를 원격으로 열어주는 쪽 (호스트). */
export default function HostPanel() {
  const [info, setInfo] = useState<HostInfo | null>(null);
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [running, setRunning] = useState(false);
  const [displayPassword, setDisplayPassword] = useState<string | null>(null);
  const [sessions, setSessions] = useState<string[]>([]);
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editingCode, setEditingCode] = useState(false);
  const [code1, setCode1] = useState('');
  const [code2, setCode2] = useState('');
  const [service, setService] = useState<{ installed: boolean; running: boolean; supported?: boolean } | null>(null);
  const [serviceBusy, setServiceBusy] = useState(false);
  const [loginAgent, setLoginAgent] = useState<{ installed: boolean; supported: boolean } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const i = await api.hostInfo();
      setInfo(i);
      setDisplayPassword(i.display_password);
      setRunning(await api.hostRunning());
      setSettings(await api.getSettings());
      setService(await api.serviceStatus().catch(() => null));
      setLoginAgent(await api.loginAgentStatus().catch(() => null));
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
      if (event === 'display_password') setDisplayPassword(data.password);
      if (event === 'session_started')
        setSessions((s) => [...s, `${data.client} (${data.peer})${data.via_access_code ? ' · 접속 코드' : ''}`]);
      if (event === 'session_ended') setSessions((s) => s.filter((x) => !x.includes(`(${data.peer})`)));
    });
    return () => {
      window.removeEventListener('focus', refresh);
      un.then((f) => f());
    };
  }, [refresh]);

  /** 설정을 바꾼 뒤 켜져 있으면 다시 시작해서 적용한다. */
  const applyAndRestart = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
      if (running) {
        await api.hostStop();
        await api.hostStart();
      }
      await refresh();
    } catch (e) {
      setError(String(e));
    }
  };

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

  const saveCode = async () => {
    if (code1.length < 10) return setError('고정 접속 코드는 10자 이상이어야 해');
    if (code1 !== code2) return setError('두 번 입력한 코드가 달라');
    await applyAndRestart(() => api.hostSetAccessCode(code1));
    setCode1('');
    setCode2('');
    setEditingCode(false);
    setNotice('고정 접속 코드를 저장했어. 이 화면에는 다시 보이지 않으니 따로 기억해둬.');
  };

  const clearCode = async () => {
    if (info && !info.allow_display_password) {
      return setError('표시 비밀번호가 꺼져 있어서 접속 코드를 지우면 아무도 접속할 수 없어. 표시 비밀번호를 먼저 켜줘.');
    }
    await applyAndRestart(() => api.hostClearAccessCode());
    setNotice('고정 접속 코드를 지웠어.');
  };

  const setSetting = async (s: Partial<{ autoHost: boolean; launchAtLogin: boolean }>) => {
    try {
      await api.setSettings(s);
      setSettings(await api.getSettings());
    } catch (e) {
      setError(String(e));
    }
  };

  const needsPermission = info && (!info.screen_capture || !info.accessibility);
  const isWindows = navigator.userAgent.includes('Windows');
  const serviceRunning = !!service?.running;

  const changeLoginAgent = async (install: boolean) => {
    setServiceBusy(true);
    setError(null);
    try {
      await api.loginAgentChange(install);
      setNotice(install ? '로그인 화면 접속을 켰어. 로그아웃한 상태에서도 고정 접속 코드로 들어올 수 있어.' : '로그인 화면 접속을 껐어.');
    } catch (e) {
      setError(String(e));
    } finally {
      setServiceBusy(false);
      refresh();
    }
  };

  const changeService = async (install: boolean) => {
    setServiceBusy(true);
    setError(null);
    try {
      // 앱 호스트와 서비스가 같은 포트를 쓰므로 설치 전에 앱 쪽 호스트를 끈다.
      if (install && running) await api.hostStop();
      await api.serviceChange(install);
      setNotice(install ? '서비스 모드를 켰어. 로그인 화면 · UAC 창까지 원격으로 볼 수 있어.' : '서비스 모드를 껐어.');
    } catch (e) {
      setError(String(e));
    } finally {
      setServiceBusy(false);
      refresh();
    }
  };

  return (
    <section className="panel">
      <div className="card hero">
        <div>
          <h2>{running ? '접속 대기 중' : '꺼져 있음'}</h2>
          <p className="muted">
            {running ? '아래 주소로 다른 컴퓨터에서 접속할 수 있어.' : '켜면 이 컴퓨터 화면을 원격으로 열어줘.'}
          </p>
        </div>
        {serviceRunning ? (
          <span className="muted small-text">서비스 모드로 대기 중</span>
        ) : (
          <button className={running ? 'danger' : 'primary'} onClick={toggle}>
            {running ? '끄기' : '켜기'}
          </button>
        )}
      </div>

      {error && <div className="card error">{error}</div>}
      {notice && (
        <div className="card notice" onClick={() => setNotice(null)}>
          {notice}
        </div>
      )}

      {needsPermission && (
        <div className="card warn">
          <strong>권한이 필요해</strong>
          <ul>
            {!info!.screen_capture && <li>화면 기록: 화면을 보내려면 필요해</li>}
            {!info!.accessibility && <li>손쉬운 사용: 원격 마우스·키보드 조작에 필요해</li>}
          </ul>
          <p className="muted">macOS 는 이 권한을 원격으로 허용할 수 없어서, 처음 한 번은 이 컴퓨터 앞에서 직접 허용해야 해.</p>
          <button onClick={() => api.hostRequestPermissions().finally(refresh)}>권한 요청</button>
        </div>
      )}

      {info && (
        <div className="card grid">
          <div className="label">주소</div>
          <div className="mono">
            {info.addresses.length ? info.addresses.map((a) => `${a}:${info.port}`).join('  ·  ') : '네트워크 없음'}
          </div>
          <div className="label">지문</div>
          <div className="mono small-text">{info.short_fingerprint}</div>
          <div className="label">접속 중</div>
          <div>{sessions.length ? sessions.join(', ') : '없음'}</div>
        </div>
      )}

      {info && (
        <div className="card">
          <h3>표시 비밀번호</h3>
          <p className="muted small-text">옆 사람에게 알려줘도 되는 임시 비밀번호. 켤 때마다 바뀌게 할 수 있어.</p>
          {info.allow_display_password ? (
            <div className="row">
              {displayPassword ? <span className="mono big">{displayPassword}</span> : <span className="muted">켜면 만들어져</span>}
              <button className="small" onClick={() => applyAndRestart(() => api.hostResetPassword())}>
                새로 만들기
              </button>
            </div>
          ) : (
            <p className="muted">꺼져 있음. 고정 접속 코드로만 접속할 수 있어.</p>
          )}
          <label className="check">
            <input
              type="checkbox"
              checked={info.rotate_display_password}
              disabled={!info.allow_display_password}
              onChange={(e) => applyAndRestart(() => api.hostConfigure({ rotate: e.target.checked }))}
            />
            켤 때마다 새로 만들기
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={info.allow_display_password}
              disabled={!info.has_access_code && info.allow_display_password}
              onChange={(e) => applyAndRestart(() => api.hostConfigure({ allowDisplay: e.target.checked }))}
            />
            표시 비밀번호로 접속 허용 {!info.has_access_code && <span className="muted small-text">(끄려면 고정 접속 코드 먼저)</span>}
          </label>
        </div>
      )}

      {info && (
        <div className="card">
          <h3>고정 접속 코드</h3>
          <p className="muted small-text">
            나만 아는 비밀. 이 컴퓨터에는 해시만 저장돼서 다시 볼 수 없고, 직접 바꾸기 전에는 바뀌지 않아. 회사 컴퓨터에서
            이 코드로 접속하고 "키체인에 저장"하면 다음부터 클릭 한 번으로 들어와.
          </p>
          {!editingCode ? (
            <div className="row">
              <span>{info.has_access_code ? '설정됨' : '없음'}</span>
              <button className="small" onClick={() => setEditingCode(true)}>
                {info.has_access_code ? '바꾸기' : '설정'}
              </button>
              {info.has_access_code && (
                <button className="small danger" onClick={clearCode}>
                  지우기
                </button>
              )}
            </div>
          ) : (
            <div className="form">
              <input type="password" placeholder="새 접속 코드 (10자 이상)" value={code1} onChange={(e) => setCode1(e.target.value)} />
              <input
                type="password"
                placeholder="한 번 더"
                value={code2}
                onChange={(e) => setCode2(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && saveCode()}
              />
              <div className="row">
                <button className="primary" onClick={saveCode} disabled={!code1 || !code2}>
                  저장
                </button>
                <button
                  onClick={() => {
                    setEditingCode(false);
                    setCode1('');
                    setCode2('');
                  }}
                >
                  취소
                </button>
              </div>
            </div>
          )}
        </div>
      )}

      {isWindows && service && (
        <div className="card">
          <h3>Windows 서비스 모드</h3>
          <p className="muted small-text">
            Windows 가 켜지면 로그인 전부터 접속을 받는다. 로그인 화면 · 잠금 화면 · UAC(관리자 허용) 창도 원격으로 보고 누를 수 있고,
            원격 메뉴의 Ctrl+Alt+Del 도 동작한다. 설치 · 제거에는 관리자 권한이 필요해.
          </p>
          <div className="row">
            <span>{service.running ? '실행 중' : service.installed ? '설치됨 (멈춤)' : '꺼짐'}</span>
            <button className="small" disabled={serviceBusy} onClick={() => changeService(!service.installed)}>
              {serviceBusy ? '처리 중…' : service.installed ? '서비스 제거' : '서비스 설치'}
            </button>
          </div>
        </div>
      )}

      {loginAgent?.supported && (
        <div className="card">
          <h3>로그인 화면에서도 접속 (macOS)</h3>
          <p className="muted small-text">
            로그아웃했거나 재부팅 뒤 로그인 화면에서도 접속을 받는다 (고정 접속 코드 필요). 관리자 암호가 필요해. FileVault 를 켠
            Mac 은 재부팅 직후 디스크 잠금 해제 화면에서는 접속할 수 없어 (macOS 가 뜨기 전 단계라서).
          </p>
          <div className="row">
            <span>{loginAgent.installed ? '켜짐' : '꺼짐'}</span>
            <button
              className="small"
              disabled={serviceBusy || (!loginAgent.installed && !info?.has_access_code)}
              onClick={() => changeLoginAgent(!loginAgent.installed)}
            >
              {loginAgent.installed ? '끄기' : '켜기'}
            </button>
            {!loginAgent.installed && !info?.has_access_code && <span className="muted small-text">고정 접속 코드를 먼저 정해줘</span>}
          </div>
        </div>
      )}

      {settings && (
        <div className="card">
          <h3>무인 접속</h3>
          <p className="muted small-text">밖에서 접속하려면 이 컴퓨터가 켜져서 기다리고 있어야 해.</p>
          <label className="check">
            <input type="checkbox" checked={settings.launch_at_login} onChange={(e) => setSetting({ launchAtLogin: e.target.checked })} />
            로그인하면 Legato 실행
          </label>
          <label className="check">
            <input type="checkbox" checked={settings.auto_host} onChange={(e) => setSetting({ autoHost: e.target.checked })} />
            Legato 가 켜지면 바로 접속 대기
          </label>
          <p className="muted small-text">
            접속 대기 중에는 Mac 이 잠들지 않게 막아 (화면은 꺼져도 됨). 외부 네트워크에서는 공유기에서 UDP {info?.port ?? 47820}{' '}
            포트포워딩이 필요하고, 가능하면 Tailscale·WireGuard 같은 VPN 안에서만 쓰는 걸 추천해.
          </p>
        </div>
      )}

      {logs.length > 0 && (
        <details className="card">
          <summary>로그</summary>
          <pre className="log">{logs.join('\n')}</pre>
        </details>
      )}
    </section>
  );
}
