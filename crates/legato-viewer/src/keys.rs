//! winit 물리 키를 W3C `KeyboardEvent.code` 이름으로 바꾼다.
//! winit 의 KeyCode 이름은 대부분 W3C 와 같고, Super 계열만 Meta 로 바꾼다.

use winit::keyboard::{KeyCode, PhysicalKey};

pub fn to_w3c(key: PhysicalKey) -> Option<String> {
    let PhysicalKey::Code(code) = key else { return None };
    let name = match code {
        KeyCode::SuperLeft => "MetaLeft".to_string(),
        KeyCode::SuperRight => "MetaRight".to_string(),
        other => format!("{other:?}"),
    };
    Some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 이름_변환() {
        assert_eq!(to_w3c(PhysicalKey::Code(KeyCode::KeyA)).as_deref(), Some("KeyA"));
        assert_eq!(to_w3c(PhysicalKey::Code(KeyCode::SuperLeft)).as_deref(), Some("MetaLeft"));
        assert_eq!(to_w3c(PhysicalKey::Code(KeyCode::ArrowUp)).as_deref(), Some("ArrowUp"));
        assert_eq!(to_w3c(PhysicalKey::Code(KeyCode::Digit1)).as_deref(), Some("Digit1"));
    }
}
