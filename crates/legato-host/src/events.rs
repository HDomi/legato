//! 표준 출력으로 내보내는 상태 이벤트 (JSON 한 줄씩). Tauri 앱이 읽어서 화면에 보여준다.
//! 사람용 로그는 표준 에러로 따로 나간다.

use std::io::Write;

pub fn emit(event: &str, data: serde_json::Value) {
    let line = serde_json::json!({ "event": event, "data": data });
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
