//! OS 사이 키 매핑. Mac 의 Cmd 와 Windows 의 Ctrl 이 같은 역할을 하도록 바꿔 준다.
//!
//! 예: Mac 뷰어 → Windows 호스트에서 ⌘C 를 누르면 Ctrl+C 가 되고, 반대로 Mac 의 Control 은 Windows 키가 된다.

use crate::protocol::Os;

/// 뷰어 OS 와 호스트 OS 가 다를 때 Cmd(Meta) 와 Control 을 서로 바꾼다.
pub fn should_swap_cmd_ctrl(viewer: Os, host: Os) -> bool {
    matches!(
        (viewer, host),
        (Os::Mac, Os::Windows) | (Os::Mac, Os::Linux) | (Os::Windows, Os::Mac) | (Os::Linux, Os::Mac)
    )
}

/// W3C 키 이름에 Cmd↔Ctrl 교환을 적용한다.
pub fn swap_cmd_ctrl(code: &str) -> &str {
    match code {
        "MetaLeft" => "ControlLeft",
        "MetaRight" => "ControlRight",
        "ControlLeft" => "MetaLeft",
        "ControlRight" => "MetaRight",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 맥에서_윈도우로는_교환() {
        assert!(should_swap_cmd_ctrl(Os::Mac, Os::Windows));
        assert!(should_swap_cmd_ctrl(Os::Windows, Os::Mac));
        assert!(!should_swap_cmd_ctrl(Os::Mac, Os::Mac));
        assert!(!should_swap_cmd_ctrl(Os::Windows, Os::Windows));
        assert_eq!(swap_cmd_ctrl("MetaLeft"), "ControlLeft");
        assert_eq!(swap_cmd_ctrl("ControlRight"), "MetaRight");
        assert_eq!(swap_cmd_ctrl("KeyC"), "KeyC");
    }
}
