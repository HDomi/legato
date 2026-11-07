import { useCallback, useEffect, useState } from 'react';
import { api, type AppInfo, type AppSettings, type UpdateInfo } from './api';
import { useI18n } from './i18n';
import UpdateDialog from './UpdateDialog';

/** 설정 탭: 언어, 업데이트, 서명 · 권한 유지 안내, 시작 옵션, 입력. */
export default function SettingsPanel({ onAutoCheckChange }: { onAutoCheckChange: (v: boolean) => void }) {
  const { t, lang, setLang } = useI18n();
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [checkError, setCheckError] = useState<string | null>(null);
  const [dialog, setDialog] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setSettings(await api.getSettings());
      setInfo(await api.appInfo());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const change = async (s: Parameters<typeof api.setSettings>[0]) => {
    setError(null);
    try {
      await api.setSettings(s);
      const next = await api.getSettings();
      setSettings(next);
      onAutoCheckChange(next.auto_update_check);
    } catch (e) {
      setError(String(e));
    }
  };

  const checkNow = async () => {
    setChecking(true);
    setCheckError(null);
    try {
      setUpdate(await api.checkUpdate());
    } catch (e) {
      setCheckError(String(e));
    } finally {
      setChecking(false);
    }
  };

  return (
    <section className="panel">
      {error && <div className="card error">{error}</div>}

      <div className="card">
        <h3>{t('settings.language')}</h3>
        <p className="muted small-text">{t('settings.languageDesc')}</p>
        <div className="row">
          <button className={lang === 'ko' ? 'primary small' : 'small'} onClick={() => setLang('ko')}>
            한국어
          </button>
          <button className={lang === 'en' ? 'primary small' : 'small'} onClick={() => setLang('en')}>
            English
          </button>
        </div>
      </div>

      <div className="card">
        <h3>{t('settings.update')}</h3>
        <div className="row">
          <span>
            {t('settings.version')}: <span className="mono">v{info?.version ?? ''}</span>
          </span>
          <button className="small" disabled={checking} onClick={checkNow}>
            {checking ? t('settings.checking') : t('settings.checkNow')}
          </button>
        </div>
        {update && (
          <div className="row">
            {update.available ? (
              <>
                <span>{t('settings.available', { version: update.latest })}</span>
                <button className="small primary" onClick={() => setDialog(true)}>
                  {t('update.install')}
                </button>
              </>
            ) : (
              <span className="muted">{t('settings.upToDate')}</span>
            )}
          </div>
        )}
        {checkError && <p className="error-text small-text">{t('update.checkFailed', { error: checkError })}</p>}
        {settings && (
          <label className="check">
            <input
              type="checkbox"
              checked={settings.auto_update_check}
              onChange={(e) => change({ autoUpdateCheck: e.target.checked })}
            />
            {t('settings.autoCheck')}
          </label>
        )}
      </div>

      {info && settings?.os === 'macos' && (
        <div className="card">
          <h3>{t('settings.signing')}</h3>
          <p className={info.adhoc ? 'warn-text small-text' : 'muted small-text'}>
            {info.adhoc ? t('settings.signedAdhoc') : t('settings.signedStable')}
          </p>
          <p className="muted small-text">{t('settings.keepData')}</p>
          <p className="muted small-text">{t('settings.keychainNote')}</p>
        </div>
      )}

      {settings && (
        <div className="card">
          <h3>{t('settings.startup')}</h3>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.launch_at_login}
              onChange={(e) => change({ launchAtLogin: e.target.checked })}
            />
            {t('settings.launchAtLogin')}
          </label>
          <label className="check">
            <input type="checkbox" checked={settings.auto_host} onChange={(e) => change({ autoHost: e.target.checked })} />
            {t('settings.autoHost')}
          </label>
          <p className="muted small-text">{t('settings.startupDesc')}</p>
        </div>
      )}

      {settings && (
        <div className="card">
          <h3>{t('settings.input')}</h3>
          <label className="check">
            <input
              type="checkbox"
              checked={settings.caps_hangul}
              onChange={(e) => change({ capsHangul: e.target.checked })}
            />
            {t('settings.capsHangul')}
          </label>
          <p className="muted small-text">{t('settings.capsHangulDesc')}</p>
        </div>
      )}

      {dialog && update && <UpdateDialog info={update} onClose={() => setDialog(false)} />}
    </section>
  );
}
