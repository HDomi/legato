import { useEffect, useState } from 'react';
import { api, type UpdateInfo } from './api';
import { useI18n } from './i18n';
import UpdateDialog from './UpdateDialog';

/** 자동 확인이 켜져 있으면 앱을 켤 때와 6시간마다 새 릴리스를 확인하고, 있으면 헤더에 배지를 띄운다. */
const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

export default function UpdateBadge({ autoCheck, onOpenSettings }: { autoCheck: boolean; onOpenSettings: () => void }) {
  const { t } = useI18n();
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [open, setOpen] = useState(false);

  useEffect(() => {
    if (!autoCheck) return;
    const check = () =>
      api
        .checkUpdate()
        .then(setInfo)
        .catch(() => {});
    check();
    const timer = setInterval(check, CHECK_INTERVAL_MS);
    return () => clearInterval(timer);
  }, [autoCheck]);

  if (!info?.available) {
    return (
      <button className="link version muted" onClick={onOpenSettings}>
        {info ? `v${info.current}` : ''}
      </button>
    );
  }
  return (
    <>
      <button className="update-badge" onClick={() => setOpen(true)}>
        {t('update.badge', { version: info.latest })}
      </button>
      {open && <UpdateDialog info={info} onClose={() => setOpen(false)} />}
    </>
  );
}
