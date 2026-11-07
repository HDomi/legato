import { useCallback, useEffect, useState } from 'react';
import { api, type HostInfo } from './api';
import { useI18n } from './i18n';

/** 이 컴퓨터를 원격으로 열어주는 쪽 (호스트). */
export default function HostPanel() {
  const { t } = useI18n();
  const [info, setInfo] = useState<HostInfo | null>(null);
  const [running, setRunning] = useState(false);
  const [displayPassword, setDisplayPassword] = useState<string | null>(null);
  const [sessions, setSessions] = useState<string[]>([]);
  const [logs, setLogs] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [editingCode, setEditingCode] = useState(false);
  const [code1, setCode1] = useState('');
  const [code2, setCode2] = useState('');
  const [service, setService] = useState<{ installed: boolean; running: boolean; stale?: boolean; supported?: boolean } | null>(null);
  const [serviceBusy, setServiceBusy] = useState(false);
  const [locked, setLocked] = useState(false);
  const [lid, setLid] = useState<{ supported: boolean; enabled: boolean } | null>(null);
  const [loginAgent, setLoginAgent] = useState<{ installed: boolean; stale?: boolean; supported: boolean } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const i = await api.hostInfo();
      setInfo(i);
      setDisplayPassword(i.display_password);
      setRunning(await api.hostRunning());
      setService(await api.serviceStatus().catch(() => null));
      setLoginAgent(await api.loginAgentStatus().catch(() => null));
      setLid(await api.lidStatus().catch(() => null));
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
      if (event === 'lock_state') setLocked(!!data.locked);
      if (event === 'display_password') setDisplayPassword(data.password);
      if (event === 'session_started')
        setSessions((s) => [...s, `${data.client} (${data.peer})${data.via_access_code ? t('host.viaCode') : ''}`]);
      if (event === 'session_ended') setLocked(false);
      if (event === 'session_ended') setSessions((s) => s.filter((x) => !x.includes(`(${data.peer})`)));
    });
    return () => {
      window.removeEventListener('focus', refresh);
      un.then((f) => f());
    };
  }, [refresh, t]);

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
    if (code1.length < 10) return setError(t('host.codeTooShort'));
    if (code1 !== code2) return setError(t('host.codeMismatch'));
    await applyAndRestart(() => api.hostSetAccessCode(code1));
    setCode1('');
    setCode2('');
    setEditingCode(false);
    setNotice(t('host.codeSaved'));
  };

  const clearCode = async () => {
    if (info && !info.allow_display_password) {
      return setError(t('host.codeClearBlocked'));
    }
    await applyAndRestart(() => api.hostClearAccessCode());
    setNotice(t('host.codeCleared'));
  };

  const needsPermission = info && (!info.screen_capture || !info.accessibility);
  const isWindows = navigator.userAgent.includes('Windows');
  const serviceRunning = !!service?.running;

  const changeLoginAgent = async (install: boolean) => {
    setServiceBusy(true);
    setError(null);
    try {
      await api.loginAgentChange(install);
      setNotice(install ? t('host.loginAgentOn') : t('host.loginAgentOffDone'));
    } catch (e) {
      setError(String(e));
    } finally {
      setServiceBusy(false);
      refresh();
    }
  };

  const changeLid = async (enable: boolean) => {
    setServiceBusy(true);
    setError(null);
    try {
      await api.lidChange(enable);
      setNotice(enable ? t('host.lidOnDone') : t('host.lidOffDone'));
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
      setNotice(install ? t('host.serviceOn') : t('host.serviceOffDone'));
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
          <h2>{running ? t('host.waiting') : t('host.off')}</h2>
          <p className="muted">{running ? t('host.waitingDesc') : t('host.offDesc')}</p>
        </div>
        {serviceRunning ? (
          <span className="muted small-text">{t('host.serviceWaiting')}</span>
        ) : (
          <button className={running ? 'danger' : 'primary'} onClick={toggle}>
            {running ? t('host.turnOff') : t('host.turnOn')}
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
          <strong>{t('host.permTitle')}</strong>
          <ul>
            {!info!.screen_capture && <li>{t('host.permScreen')}</li>}
            {!info!.accessibility && <li>{t('host.permAx')}</li>}
          </ul>
          <p className="muted">{t('host.permDesc')}</p>
          <button onClick={() => api.hostRequestPermissions().finally(refresh)}>{t('host.permRequest')}</button>
        </div>
      )}

      {info && (
        <div className="card grid">
          <div className="label">{t('host.address')}</div>
          <div className="mono">
            {info.addresses.length ? info.addresses.map((a) => `${a}:${info.port}`).join('  ·  ') : t('host.noNetwork')}
          </div>
          <div className="label">{t('host.fingerprint')}</div>
          <div className="mono small-text">{info.short_fingerprint}</div>
          <div className="label">{t('host.connected')}</div>
          <div>
            {sessions.length ? sessions.join(', ') : t('host.none')}
            {locked && sessions.length > 0 && <span className="tag">🔒 {t('host.locked')}</span>}
          </div>
        </div>
      )}

      {info && (
        <div className="card">
          <h3>{t('host.displayPw')}</h3>
          <p className="muted small-text">{t('host.displayPwDesc')}</p>
          {info.allow_display_password ? (
            <div className="row">
              {displayPassword ? (
                <span className="mono big">{displayPassword}</span>
              ) : (
                <span className="muted">{t('host.displayPwPending')}</span>
              )}
              <button className="small" onClick={() => applyAndRestart(() => api.hostResetPassword())}>
                {t('host.regenerate')}
              </button>
            </div>
          ) : (
            <p className="muted">{t('host.displayPwOff')}</p>
          )}
          <label className="check">
            <input
              type="checkbox"
              checked={info.rotate_display_password}
              disabled={!info.allow_display_password}
              onChange={(e) => applyAndRestart(() => api.hostConfigure({ rotate: e.target.checked }))}
            />
            {t('host.rotate')}
          </label>
          <label className="check">
            <input
              type="checkbox"
              checked={info.allow_display_password}
              disabled={!info.has_access_code && info.allow_display_password}
              onChange={(e) => applyAndRestart(() => api.hostConfigure({ allowDisplay: e.target.checked }))}
            />
            {t('host.allowDisplay')}{' '}
            {!info.has_access_code && <span className="muted small-text">{t('host.allowDisplayNeedsCode')}</span>}
          </label>
        </div>
      )}

      {info && (
        <div className="card">
          <h3>{t('host.code')}</h3>
          <p className="muted small-text">{t('host.codeDesc')}</p>
          {!editingCode ? (
            <div className="row">
              <span>{info.has_access_code ? t('host.codeSet') : t('host.none')}</span>
              <button className="small" onClick={() => setEditingCode(true)}>
                {info.has_access_code ? t('host.change') : t('host.set')}
              </button>
              {info.has_access_code && (
                <button className="small danger" onClick={clearCode}>
                  {t('host.clear')}
                </button>
              )}
            </div>
          ) : (
            <div className="form">
              <input type="password" placeholder={t('host.newCode')} value={code1} onChange={(e) => setCode1(e.target.value)} />
              <input
                type="password"
                placeholder={t('host.again')}
                value={code2}
                onChange={(e) => setCode2(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && saveCode()}
              />
              <div className="row">
                <button className="primary" onClick={saveCode} disabled={!code1 || !code2}>
                  {t('host.save')}
                </button>
                <button
                  onClick={() => {
                    setEditingCode(false);
                    setCode1('');
                    setCode2('');
                  }}
                >
                  {t('host.cancel')}
                </button>
              </div>
            </div>
          )}
        </div>
      )}

      {isWindows && service && (
        <div className="card">
          <h3>{t('host.service')}</h3>
          <p className="muted small-text">{t('host.serviceDesc')}</p>
          <div className="row">
            <span>
              {service.running ? t('host.serviceRunning') : service.installed ? t('host.serviceStopped') : t('host.serviceOff')}
            </span>
            <button className="small" disabled={serviceBusy} onClick={() => changeService(!service.installed)}>
              {serviceBusy ? t('host.working') : service.installed ? t('host.serviceRemove') : t('host.serviceInstall')}
            </button>
          </div>
          {service.installed && service.stale && (
            <div className="row">
              <span className="muted small-text">{t('host.serviceStale')}</span>
              <button className="small primary" disabled={serviceBusy} onClick={() => changeService(true)}>
                {t('host.loginAgentReapply')}
              </button>
            </div>
          )}
        </div>
      )}

      {loginAgent?.supported && (
        <div className="card">
          <h3>{t('host.loginAgent')}</h3>
          <p className="muted small-text">{t('host.loginAgentDesc')}</p>
          <div className="row">
            <span>{loginAgent.installed ? t('host.on') : t('host.offState')}</span>
            <button
              className="small"
              disabled={serviceBusy || (!loginAgent.installed && !info?.has_access_code)}
              onClick={() => changeLoginAgent(!loginAgent.installed)}
            >
              {loginAgent.installed ? t('host.turnOff') : t('host.turnOn')}
            </button>
            {!loginAgent.installed && !info?.has_access_code && (
              <span className="muted small-text">{t('host.needCodeFirst')}</span>
            )}
          </div>
          {loginAgent.installed && loginAgent.stale && (
            <div className="row">
              <span className="muted small-text">{t('host.loginAgentStale')}</span>
              <button className="small primary" disabled={serviceBusy} onClick={() => changeLoginAgent(true)}>
                {t('host.loginAgentReapply')}
              </button>
            </div>
          )}
        </div>
      )}

      {lid?.supported && (
        <div className="card">
          <h3>{t('host.lid')}</h3>
          <p className="muted small-text">{t('host.lidDesc')}</p>
          {lid.enabled && <p className="warn-text small-text">{t('host.lidWarn')}</p>}
          <div className="row">
            <span>{lid.enabled ? t('host.on') : t('host.offState')}</span>
            <button className="small" disabled={serviceBusy} onClick={() => changeLid(!lid.enabled)}>
              {lid.enabled ? t('host.turnOff') : t('host.turnOn')}
            </button>
          </div>
        </div>
      )}

      <p className="muted small-text">{t('host.network', { port: info?.port ?? 47820 })}</p>

      {logs.length > 0 && (
        <details className="card">
          <summary>{t('host.logs')}</summary>
          <pre className="log">{logs.join('\n')}</pre>
        </details>
      )}
    </section>
  );
}
