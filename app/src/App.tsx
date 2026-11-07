import { useCallback, useEffect, useState } from 'react';
import HostPanel from './HostPanel';
import ConnectPanel from './ConnectPanel';
import SettingsPanel from './SettingsPanel';
import UpdateBadge from './UpdateBadge';
import { api } from './api';
import { LangProvider, useI18n, type Lang } from './i18n';

type Tab = 'connect' | 'host' | 'settings';

export default function App() {
  const [lang, setLangState] = useState<Lang>('ko');
  const [autoCheck, setAutoCheck] = useState(true);

  useEffect(() => {
    api
      .getSettings()
      .then((s) => {
        setLangState(s.lang === 'en' ? 'en' : 'ko');
        setAutoCheck(s.auto_update_check);
      })
      .catch(() => {});
  }, []);

  const setLang = useCallback((l: Lang) => {
    setLangState(l);
    document.documentElement.lang = l;
    api.setSettings({ lang: l }).catch(() => {});
  }, []);

  return (
    <LangProvider lang={lang} setLang={setLang}>
      <Shell autoCheck={autoCheck} onAutoCheckChange={setAutoCheck} />
    </LangProvider>
  );
}

function Shell({ autoCheck, onAutoCheckChange }: { autoCheck: boolean; onAutoCheckChange: (v: boolean) => void }) {
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>('connect');
  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <img className="logo" src="/icon.png" alt="" />
          <span>Legato</span>
          <UpdateBadge autoCheck={autoCheck} onOpenSettings={() => setTab('settings')} />
        </div>
        <nav className="tabs">
          <button className={tab === 'connect' ? 'active' : ''} onClick={() => setTab('connect')}>
            {t('tab.connect')}
          </button>
          <button className={tab === 'host' ? 'active' : ''} onClick={() => setTab('host')}>
            {t('tab.host')}
          </button>
          <button className={tab === 'settings' ? 'active' : ''} onClick={() => setTab('settings')}>
            {t('tab.settings')}
          </button>
        </nav>
      </header>
      <main className="main">
        {/* 모든 패널을 마운트해 두어 탭을 바꿔도 상태 · 로그가 유지되게 한다. */}
        <div hidden={tab !== 'connect'}>
          <ConnectPanel />
        </div>
        <div hidden={tab !== 'host'}>
          <HostPanel />
        </div>
        <div hidden={tab !== 'settings'}>
          <SettingsPanel onAutoCheckChange={onAutoCheckChange} />
        </div>
      </main>
    </div>
  );
}
