import { useState } from 'react';
import { api, type UpdateInfo } from './api';
import { useI18n } from './i18n';

/** 업데이트 확인 창: 노트, 주의 사항, 설치 버튼. 헤더 배지와 설정 탭에서 같이 쓴다. */
export default function UpdateDialog({ info, onClose }: { info: UpdateInfo; onClose: () => void }) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const install = async (force: boolean) => {
    setBusy(true);
    setError(null);
    try {
      // 성공하면 앱이 곧 꺼졌다가 새 버전으로 다시 열린다.
      await api.installUpdate(info.latest, force);
    } catch (e) {
      const msg = String(e);
      if (msg.startsWith('BUSY') && !force) {
        if (confirm(t('update.busyConfirm', { reason: msg.replace(/^BUSY:\s*/, '') }))) {
          return install(true);
        }
      } else {
        setError(msg);
      }
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onClick={() => !busy && onClose()}>
      <div className="modal card" onClick={(e) => e.stopPropagation()}>
        <h3>
          v{info.current} → v{info.latest}
        </h3>
        {info.notes ? (
          <pre className="notes">{info.notes}</pre>
        ) : (
          <p className="muted small-text">{t('update.noNotes', { url: info.url })}</p>
        )}
        {info.permissions_reset && <p className="warn-text small-text">{t('update.permReset')}</p>}
        {!info.has_asset && <p className="warn-text small-text">{t('update.noAsset')}</p>}
        {error && <p className="error-text small-text">{error}</p>}
        <div className="row">
          <button className="primary" disabled={busy || !info.has_asset} onClick={() => install(false)}>
            {busy ? t('update.downloading') : t('update.install')}
          </button>
          <button disabled={busy} onClick={onClose}>
            {t('update.later')}
          </button>
        </div>
      </div>
    </div>
  );
}
