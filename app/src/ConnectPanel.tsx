import { useEffect, useRef, useState } from 'react';
import { api, type Entry } from './api';

interface SessionState {
  id: string;
  address: string;
  status: 'connecting' | 'connected' | 'closed';
  message?: string;
}

/** 다른 컴퓨터에 접속하는 쪽 (뷰어). */
export default function ConnectPanel() {
  const [address, setAddress] = useState('');
  const [name, setName] = useState('');
  const [password, setPassword] = useState('');
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
          if (l.kind === 'exit') return s.status === 'closed' ? s : { ...s, status: 'closed' };
          if (l.kind === 'log') {
            const line = String(l.value);
            return line.startsWith('오류:') ? { ...s, message: line.replace(/^오류:\s*/, '') } : s;
          }
          const { event, data } = l.value;
          if (event === 'connected') return { ...s, status: 'connected', message: `${data.host} 에 연결됨` };
          if (event === 'notice') return { ...s, message: data.message };
          if (event === 'closed') return { ...s, status: 'closed', message: data.error ?? '연결 끝' };
          return s;
        }),
      );
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const saveBook = (next: Entry[]) => {
    setBook(next);
    api.saveAddresses(next).catch(() => {});
  };

  const connect = async (addr: string, acceptNew = false) => {
    if (!addr.trim() || !password) return;
    const id = `s${++counter.current}`;
    setSessions((prev) => [{ id, address: addr, status: 'connecting' }, ...prev.slice(0, 9)]);
    try {
      await api.connect(id, addr.trim(), password, windowed, acceptNew);
    } catch (e) {
      setSessions((prev) => prev.map((s) => (s.id === id ? { ...s, status: 'closed', message: String(e) } : s)));
    }
  };

  const addToBook = () => {
    if (!address.trim()) return;
    const entry = { name: name.trim() || address.trim(), address: address.trim() };
    saveBook([...book.filter((e) => e.address !== entry.address), entry]);
    setName('');
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
            비밀번호
            <input
              type="password"
              placeholder="호스트에 표시된 비밀번호"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              onKeyDown={(e) => e.key === 'Enter' && connect(address)}
            />
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
                  <strong>{e.name}</strong> <span className="mono muted">{e.address}</span>
                </button>
                <div className="row">
                  <button className="small primary" disabled={!password} onClick={() => connect(e.address)}>
                    접속
                  </button>
                  <button className="small" onClick={() => saveBook(book.filter((x) => x.address !== e.address))}>
                    삭제
                  </button>
                </div>
              </li>
            ))}
          </ul>
          {!password && <p className="muted small-text">비밀번호를 입력하면 바로 접속할 수 있어.</p>}
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
                  <button className="small danger" onClick={() => connect(s.address, true)}>
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
