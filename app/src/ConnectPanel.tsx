import { useEffect, useRef, useState } from 'react';
import { api, type Entry } from './api';

interface SessionState {
  id: string;
  address: string;
  status: 'connecting' | 'connected' | 'closed';
  message?: string;
  useSaved: boolean;
}

/** 다른 컴퓨터에 접속하는 쪽 (뷰어). */
export default function ConnectPanel() {
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
          if (l.kind === 'exit') return s.status === 'closed' ? s : { ...s, status: 'closed', message: '연결 끝' };
          if (l.kind === 'log') {
            const line = String(l.value);
            if (line.startsWith('알림:')) return { ...s, message: line.replace(/^알림:\s*/, '') };
            return line.startsWith('오류:') ? { ...s, message: line.replace(/^오류:\s*/, '') } : s;
          }
          const { event, data } = l.value;
          if (event === 'connected')
            return { ...s, status: 'connected', message: `${data.host} 에 연결됨${data.via_access_code ? ' (접속 코드)' : ''}` };
          if (event === 'notice') return { ...s, message: data.message };
          if (event === 'closed') return { ...s, status: 'closed', message: data.error ?? '연결 끝' };
          return s;
        }),
      );
    });
    const un2 = api.onCodeSaved((p) => {
      if (p.error) {
        setSessions((prev) => prev.map((s) => (s.id === p.tag ? { ...s, message: `${s.message ?? ''} · 저장 안 함: ${p.error}` } : s)));
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
  }, []);

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
            주소
            <input
              placeholder="192.168.0.10 또는 192.168.0.10:47820"
              value={address}
              onChange={(e) => setAddress(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && connect(address)}
            />
          </label>
          <label>
            비밀번호 또는 접속 코드
            <input
              type="password"
              placeholder="표시 비밀번호나 고정 접속 코드"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && connect(address)}
            />
          </label>
          <label className="check">
            <input type="checkbox" checked={remember} onChange={(e) => setRemember(e.target.checked)} />
            고정 접속 코드면 {isMac ? '키체인' : '자격 증명 관리자'}에 저장 (다음부터 바로 접속)
          </label>
          {isMac && (
            <label className="check">
              <input type="checkbox" checked={windowed} onChange={(e) => setWindowed(e.target.checked)} />
              전체화면 Space 대신 창으로 열기
            </label>
          )}
          <div className="row">
            <button className="primary" onClick={() => connect(address)} disabled={!address.trim() || !password}>
              접속
            </button>
            <input className="grow" placeholder="이름 (주소록 저장용)" value={name} onChange={(e) => setName(e.target.value)} />
            <button onClick={addToBook} disabled={!address.trim()}>
              주소록에 추가
            </button>
          </div>
        </div>
      </div>

      {book.length > 0 && (
        <div className="card">
          <h3>주소록</h3>
          <ul className="list">
            {book.map((e) => (
              <li key={e.address}>
                <button className="link" onClick={() => setAddress(e.address)}>
                  <strong>{e.name}</strong> {e.name !== e.address && <span className="mono muted">{e.address}</span>}
                  {e.saved_code && <span className="tag">코드 저장됨</span>}
                </button>
                <div className="row">
                  {e.saved_code ? (
                    <>
                      <button className="small primary" onClick={() => connect(e.address, { useSaved: true })}>
                        바로 접속
                      </button>
                      <button className="small" onClick={() => forget(e.address)}>
                        코드 지우기
                      </button>
                    </>
                  ) : (
                    <button className="small primary" disabled={!password} onClick={() => connect(e.address)}>
                      접속
                    </button>
                  )}
                  <button
                    className="small"
                    onClick={async () => {
                      if (e.saved_code) await api.forgetCode(e.address).catch(() => {});
                      saveBook(book.filter((x) => x.address !== e.address));
                    }}
                  >
                    삭제
                  </button>
                </div>
              </li>
            ))}
          </ul>
        </div>
      )}

      {sessions.length > 0 && (
        <div className="card">
          <h3>세션</h3>
          <ul className="list">
            {sessions.map((s) => (
              <li key={s.id}>
                <div>
                  <span className={`dot ${s.status}`} /> <span className="mono">{s.address}</span>
                  {s.message && <div className="muted small-text">{s.message}</div>}
                </div>
                {s.status === 'closed' && s.message?.includes('지문이 예전과 달라') && (
                  <button className="small danger" onClick={() => connect(s.address, { useSaved: s.useSaved, acceptNew: true })}>
                    새 지문 믿고 다시 접속
                  </button>
                )}
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}
