//! 사용자에게 보이는 문구의 언어 (한국어 · 영어). 기본은 한국어.
//!
//! 언어는 환경 변수 `LEGATO_LANG` (`ko` / `en`) 으로 정한다. 앱이 호스트·뷰어를 띄울 때 넘겨준다.
//! 원격으로 보내는 안내 문구는 받는 쪽(뷰어)이 접속할 때 알려준 언어로 만든다.
//! 개발용 로그(tracing)는 번역하지 않는다.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    Ko,
    En,
}

impl Lang {
    /// `en`, `en-US` 같은 값은 영어, 그 밖은 한국어.
    pub fn parse(s: &str) -> Self {
        if s.trim().to_ascii_lowercase().starts_with("en") { Lang::En } else { Lang::Ko }
    }

    pub fn code(self) -> &'static str {
        match self {
            Lang::Ko => "ko",
            Lang::En => "en",
        }
    }

    /// 이 언어의 문구를 고른다.
    pub fn pick<'a>(self, ko: &'a str, en: &'a str) -> &'a str {
        match self {
            Lang::Ko => ko,
            Lang::En => en,
        }
    }
}

/// 이 프로세스의 언어 (`LEGATO_LANG`, 없으면 한국어).
pub fn lang() -> Lang {
    static L: OnceLock<Lang> = OnceLock::new();
    *L.get_or_init(|| std::env::var("LEGATO_LANG").map(|v| Lang::parse(&v)).unwrap_or_default())
}

/// 이 프로세스 언어로 문구를 고른다: `t!("한국어", "English")`.
#[macro_export]
macro_rules! t {
    ($ko:expr, $en:expr $(,)?) => {
        $crate::i18n::lang().pick($ko, $en)
    };
}

#[cfg(test)]
mod tests {
    use super::Lang;

    #[test]
    fn 언어_해석() {
        assert_eq!(Lang::parse("en-US"), Lang::En);
        assert_eq!(Lang::parse("EN"), Lang::En);
        assert_eq!(Lang::parse("ko"), Lang::Ko);
        assert_eq!(Lang::parse(""), Lang::Ko);
        assert_eq!(Lang::En.pick("가", "a"), "a");
    }
}
