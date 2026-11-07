import { useEffect, useState } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { api, type UpdateInfo } from './api';

/** 6시간마다 새 릴리스를 확인하고, 있으면 헤더에 배지를 띄운다. */
const CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

export default function UpdateBadge() {
  const [info, setInfo] = useState<UpdateInfo | null>(null);
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [version, setVersion] = useState<string>('');
  const [checkError, setCheckError] = useState<string | null>(null);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
    const check = () =>
      api
        .checkUpdate()
        .then((i) => {
          setInfo(i);
          setCheckError(null);
        })
        .catch((e) => setCheckError(String(e)));
    check();
    const t = setInterval(check, CHECK_INTERVAL_MS);
    return () => clearInterval(t);
  }, []);

  if (!info || !info.available) {
    // 확인에 실패해도 현재 버전은 보여준다 (마우스를 올리면 이유).
    return (
      <span className="version muted" title={checkError ? `업데이트 확인 실패: ${checkError}` : '최신 버전'}>
        v{info?.current ?? version}
      </span>
    );
  }

  const install = async (force: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await api.installUpdate(info.latest, force);
      // 성공하면 앱이 곧 꺼졌다가 새 버전으로 다시 열린다.
    } catch (e) {
      const msg = String(e);
      if (msg.startsWith('BUSY') && !force) {
        if (confirm(`${msg.replace(/^BUSY:\s*/, '')}\n업데이트하면 연결이 끊겨. 그래도 할래?`)) {
          return install(true);
        }
      } else {
        setError(msg);
      }
      setBusy(false);
    }
  };

  return (
    <>
      <button className="update-badge" onClick={() => setOpen(true)}>
        v{info.latest} 업데이트
      </button>
      {open && (
        <div className="modal-backdrop" onClick={() => !busy && setOpen(false)}>
          <div className="modal card" onClick={(e) => e.stopPropagation()}>
            <h3>
              v{info.current} → v{info.latest}
            </h3>
            {info.notes ? (
              <pre className="notes">{info.notes}</pre>
            ) : (
              <p className="muted small-text">릴리스 노트를 가져오지 못했어. 릴리스 페이지에서 볼 수 있어: {info.url}</p>
            )}
            {info.permissions_reset && (
              <p className="warn-text small-text">
                지금 앱은 임시(ad-hoc) 서명이라 업데이트하면 화면 기록 · 손쉬운 사용 권한을 이 컴퓨터 앞에서 다시 허용해야 해. 원격으로만
                쓰는 중이면 업데이트 뒤 접속이 안 될 수 있어.
              </p>
            )}
            {!info.has_asset && <p className="warn-text small-text">이 기기용 설치 파일이 릴리스에 없어. 릴리스 페이지에서 직접 받아줘.</p>}
            {error && <p className="error-text small-text">{error}</p>}
            <div className="row">
              <button className="primary" disabled={busy || !info.has_asset} onClick={() => install(false)}>
                {busy ? '받는 중…' : '업데이트하고 다시 열기'}
              </button>
              <button disabled={busy} onClick={() => setOpen(false)}>
                나중에
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
