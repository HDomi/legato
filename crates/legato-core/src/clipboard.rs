//! 텍스트 클립보드 동기화. 전용 스레드가 클립보드를 소유하고 0.5초마다 바뀌었는지 본다.
//!
//! - 로컬에서 바뀐 텍스트는 `on_change` 콜백으로 알린다 (상대에게 보내기).
//! - 상대에게서 받은 텍스트는 `set` 으로 넣는다. 방금 넣은 값은 다시 알리지 않아 핑퐁을 막는다.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use crate::protocol::MAX_CLIPBOARD_BYTES;

/// 클립보드 스레드 핸들. drop 하면 스레드가 끝난다.
pub struct ClipboardSync {
    tx: Sender<String>,
}

impl ClipboardSync {
    /// 동기화 스레드를 시작한다. 시작 시점의 클립보드 내용은 보내지 않는다 (접속하자마자 내용이 넘어가지 않게).
    pub fn start(on_change: impl Fn(String) + Send + 'static) -> Option<Self> {
        let (tx, rx) = channel::<String>();
        std::thread::Builder::new().name("legato-clipboard".into()).spawn(move || run(rx, on_change)).ok()?;
        Some(Self { tx })
    }

    /// 상대에게서 받은 텍스트를 로컬 클립보드에 넣는다.
    pub fn set(&self, text: String) {
        let _ = self.tx.send(text);
    }

    /// 다른 스레드에서 `set` 하려고 쓰는 핸들.
    pub fn sender(&self) -> ClipboardSetter {
        ClipboardSetter { tx: self.tx.clone() }
    }
}

/// 클립보드에 넣기만 하는 가벼운 핸들 (Send).
#[derive(Clone)]
pub struct ClipboardSetter {
    tx: Sender<String>,
}

impl ClipboardSetter {
    pub fn set(&self, text: String) {
        let _ = self.tx.send(text);
    }
}

fn run(rx: Receiver<String>, on_change: impl Fn(String)) {
    let mut cb = match arboard::Clipboard::new() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("클립보드를 열 수 없어: {e}");
            return;
        }
    };
    let mut last = cb.get_text().unwrap_or_default();
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(text) => {
                if text != last && cb.set_text(text.clone()).is_ok() {
                    last = text;
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        let Ok(now) = cb.get_text() else { continue };
        if now != last {
            last = now.clone();
            if !now.is_empty() && now.len() <= MAX_CLIPBOARD_BYTES {
                on_change(now);
            }
        }
    }
}
