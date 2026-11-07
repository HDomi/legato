import { useEffect, useRef, useState } from 'react';
import { api, type Entry } from './api';
import { useI18n } from './i18n';

interface SessionState {
  id: string;
  address: string;
  status: 'connecting' | 'connected' | 'closed';
  message?: string;
  useSaved: boolean;
  /** 호스트 인증서 지문이 바뀌어서 끊겼는지. */
  fingerprintChanged?: boolean;
}

/** 다른 컴퓨터에 접속하는 쪽 (뷰어). */
export default function ConnectPanel() {
  const { t } = useI18n();
  const [address, setAddress] = useState('');
  const [name, setName] = useState('');
  const [password, setPassword] = useState('');
  const [remember, setRemember] = useState(false);
  const [windowed, setWindowed] = useState(false);
  const [book, setBook] = useState<Entry[]>([]);
  const [sessions, setSessions] = useState<SessionState[]>([]);
  const counter = useRef(0);
  const isMac = navigator.userAgent.includes('Mac');

  useEffect(() => {
    api.loadAddresses().then(setBook).catch(() => setBook([]));
    const un = api.onViewer((l) => {
      setSessions((prev) =>
        prev.map((s) => {
          if (s.id !== l.tag) return s;
          // 뷰어가 closed 이벤트 없이 끝났으면 (창 강제 종료 등) 연결 문구를 남기지 않는다.
          if (l.kind === 'exit') return s.status === 'closed' ? s : { ...s, status: 'closed', message: t('connect.ended') };
          // 사람용 로그는 무시하고 JSON 이벤트만 본다 (언어와 무관하게).
          if (l.kind === 'log') return s;
          const { event, data } = l.value;
          if (event === 'connected')
            return {
              ...s,
              status: 'connected',
              message: t('connect.connectedTo', { host: data.host }) + (data.via_access_code ? t('connect.viaCode') : ''),
            };
          if (event === 'notice') return { ...s, message: data.message };
          if (event === 'lock_state' && data.locked) return { ...s, message: t('connect.hostLocked') };
          if (event === 'closed')
            return {
              ...s,
              status: 'closed',
              message: data.error ?? t('connect.ended'),
              fingerprintChanged: data.code === 'fingerprint_changed',
            };
          return s;
        }),
      );
    });
    const un2 = api.onCodeSaved((p) => {
      if (p.error) {
        setSessions((prev) => prev.map((s) => (s.id === p.tag ? { ...s, message: `${s.message ?? ''} · ${t('connect.notSaved', { reason: p.error ?? '' })}` } : s)));
        return;
      }
      // 저장한 주소는 주소록에 넣고 "코드 저장됨" 으로 표시한다.
      setBook((prev) => {
        const next = prev.some((e) => e.address === p.address)
          ? prev.map((e) => (e.address === p.address ? { ...e, saved_code: true } : e))
          : [...prev, { name: p.address, address: p.address, saved_code: true }];
        api.saveAddresses(next).catch(() => {});
        return next;
      });
    });
    return () => {
      un.then((f) => f());
      un2.then((f) => f());
    };
  }, [t]);

  const saveBook = (next: Entry[]) => {
    setBook(next);
    api.saveAddresses(next).catch(() => {});
  };

  const connect = async (addr: string, opts: { useSaved?: boolean; acceptNew?: boolean } = {}) => {
    const useSaved = !!opts.useSaved;
    if (!addr.trim() || (!useSaved && !password)) return;
    const id = `s${++counter.current}`;
    setSessions((prev) => [{ id, address: addr, status: 'connecting', useSaved }, ...prev.slice(0, 9)]);
    try {
      await api.connect({
        id,
        address: addr.trim(),
        password: useSaved ? '' : password,
        useSaved,
        remember: !useSaved && remember,
        windowed,
        acceptNewFingerprint: !!opts.acceptNew,
      });
    } catch (e) {
      setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, status: 'closed', message: String(e) } : s)));
    }
  };

  const addToBook = () => {
    if (!address.trim()) return;
    const prev = book.find((e) => e.address === address.trim());
    const entry = { name: name.trim() || address.trim(), address: address.trim(), saved_code: prev?.saved_code ?? false };
    saveBook([...book.filter((e) => e.address !== entry.address), entry]);
    setName('');
  };

  const forget = async (addr: string) => {
    await api.forgetCode(addr);
    saveBook(book.map((e) => (e.address === addr ? { ...e, saved_code: false } : e)));
  };

  return (
    <section className="panel">
      <div className="card">
        <div className="form">
          <label>
            {t('connect.address')}
            <input
              placeholder={t('connect.addressPlaceholder')}
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && connect(address)}
            />
          </label>
          <label>
            {t('connect.secret')}
            <input
              type="password"
              placeholder={t('connect.secretPlaceholder')}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && connect(address)}
            />
          </label>
          <label className="check">
            <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
            {isMac ? t('connect.remember.mac') : t('connect.remember.win')}
          </label>
          {isMac && (
            <label className="check">
              <input type="checkbox" checked={windowed} onChange={(e) => setWindowed(e.target.checked)} />
              {t('connect.windowed')}
            </label>
          )}
          <div className="row">
            <button className="primary" onClick={() => connect(address)} disabled={!address.trim() || !password}>
              {t('connect.go')}
            </button>
            <input className="grow" placeholder={t('connect.namePlaceholder')} value={name} onChange={(e) => setName(e.target.value)} />
            <button onClick={addToBook} disabled={!address.trim()}>
              {t('connect.addToBook')}
            </button>
          </div>
        </div>
      </div>

      {book.length > 0 && (
        <div className="card">
          <h3>{t('connect.book')}</h3>
          <ul className="list">
            {book.map((e) => (
              <li key={e.address}>
                <button className="link" onClick={() => setAddress(e.address)}>
                  <strong>{e.name}</strong> {e.name !== e.address && <span className="mono muted">{e.address}</span>}
                  {e.saved_code && <span className="tag">{t('connect.saved')}</span>}
                </button>
                <div className="row">
                  {e.saved_code ? (
                    <>
                      <button className="small primary" onClick={() => connect(e.address, { useSaved: true })}>
                        {t('connect.quick')}
                      </button>
                      <button className="small" onClick={() => forget(e.address)}>
                        {t('connect.forget')}
                      </button>
                    </>
                  ) : (
                    <button className="small primary" disabled={!password} onClick={() => connect(e.address)}>
                      {t('connect.go')}
                    </button>
                  )}
                  <button
                    className="small"
                    onClick={async () => {
                      if (e.saved_code) await api.forgetCode(e.address).catch(() => {});
                      saveBook(book.filter((x) => x.address !== e.address));
                    }}
                  >
                    {t('connect.delete')}
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}

      {sessions.length > 0 && (
        <div className="card">
          <h3>{t('connect.sessions')}</h3>
          <ul className="list">
            {sessions.map((s) => (
              <li key={s.id}>
                <div>
                  <span className={`dot ${s.status}`} /> <span className="mono">{s.address}</span>
                  {s.message && <div className="muted small-text">{s.message}</div>}
                </div>
                {s.status === 'closed' && s.fingerprintChanged && !s.useSaved && password && (
                  <button className="small danger" onClick={() => connect(s.address, { acceptNew: true })}>
                    {t('connect.trustNew')}
                  </button>
                )}
                {s.status === 'closed' && s.fingerprintChanged && (s.useSaved || !password) && (
                  <span className="muted small-text">{t('connect.typeCodeToTrust')}</span>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
