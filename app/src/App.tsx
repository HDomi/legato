import { useState } from 'react';
import HostPanel from './HostPanel';
import ConnectPanel from './ConnectPanel';
import UpdateBadge from './UpdateBadge';

type Tab = 'connect' | 'host';

export default function App() {
  const [tab, setTab] = useState<Tab>('connect');
  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <img className="logo" src="/icon.png" alt="" />
          <span>Legato</span>
          <UpdateBadge />
        </div>
        <nav className="tabs">
          <button className={tab === 'connect' ? 'active' : ''} onClick={() => setTab('connect')}>
            접속하기
          </button>
          <button className={tab === 'host' ? 'active' : ''} onClick={() => setTab('host')}>
            이 컴퓨터 열기
          </button>
        </nav>
      </header>
      <main className="main">
        {/* 두 패널 모두 마운트해 두어 탭을 바꿔도 상태·로그가 유지되게 한다. */}
        <div hidden={tab !== 'connect'}>
          <ConnectPanel />
        </div>
        <div hidden={tab !== 'host'}>
          <HostPanel />
        </div>
      </main>
    </div>
  );
}
