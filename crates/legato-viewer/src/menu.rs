//! 뷰어 메뉴 막대 (muda). macOS 는 앱 메뉴 막대, Windows 는 창마다 붙는 메뉴.
//!
//! 기본 메뉴(⌘Q 종료 등)를 쓰지 않으므로 ⌘Q 같은 단축키가 원격으로 전달된다. 연결 끊기는 메뉴에서.

use legato_core::protocol::{DisplayInfo, Quality};
use muda::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};

pub const ID_DISCONNECT: &str = "disconnect";
pub const ID_CAD: &str = "cad";
pub const ID_SWAP: &str = "swap";
pub const ID_CLIPBOARD: &str = "clipboard";
pub const ID_CAPS_HANGUL: &str = "caps_hangul";
pub const ID_QUALITY_PREFIX: &str = "quality:";
pub const ID_DISPLAY_PREFIX: &str = "display:";

pub const QUALITIES: [Quality; 4] = [Quality::Auto, Quality::High, Quality::Balanced, Quality::Low];

pub fn quality_id(q: Quality) -> String {
    format!("{ID_QUALITY_PREFIX}{q:?}")
}

pub struct ViewerMenu {
    pub menu: Menu,
    /// 연결 상태 표시 (누를 수 없는 항목, 1초마다 갱신).
    pub status: MenuItem,
    pub swap: CheckMenuItem,
    pub clipboard: CheckMenuItem,
    pub caps_hangul: CheckMenuItem,
    pub qualities: Vec<(Quality, CheckMenuItem)>,
    pub displays: Vec<(u32, CheckMenuItem)>,
}

impl ViewerMenu {
    pub fn new(
        host_name: &str,
        quality: Quality,
        swap: bool,
        caps_hangul: bool,
        displays: &[DisplayInfo],
        open: &[u32],
    ) -> muda::Result<Self> {
        let menu = Menu::new();
        // macOS: 첫 하위 메뉴가 앱 메뉴 자리다.
        let app = Submenu::with_items(
            "Legato",
            true,
            &[
                &MenuItem::with_id(ID_DISCONNECT, format!("{host_name} 연결 끊기"), true, None),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(Some("Legato 가리기")),
            ],
        )?;
        let swap_item = CheckMenuItem::with_id(ID_SWAP, "Cmd ↔ Ctrl 바꾸기", true, swap, None);
        let clip_item = CheckMenuItem::with_id(ID_CLIPBOARD, "클립보드 공유", true, true, None);
        let caps_item = CheckMenuItem::with_id(ID_CAPS_HANGUL, "Caps Lock 을 한/영 키로", true, caps_hangul, None);
        let quality_menu = Submenu::new("화질", true);
        let mut qualities = Vec::new();
        for q in QUALITIES {
            let item = CheckMenuItem::with_id(quality_id(q), q.label(), true, q == quality, None);
            quality_menu.append(&item)?;
            qualities.push((q, item));
        }
        let display_menu = Submenu::new("디스플레이", true);
        let mut display_items = Vec::new();
        for (i, d) in displays.iter().enumerate() {
            let label = format!("{} ({}x{}){}", d.name, d.width, d.height, if d.primary { " · 주" } else { "" });
            let item =
                CheckMenuItem::with_id(format!("{ID_DISPLAY_PREFIX}{}", d.id), label, true, open.contains(&d.id), None);
            display_menu.append(&item)?;
            display_items.push((d.id, item));
            let _ = i;
        }
        let status = MenuItem::with_id("status", "연결 상태 확인 중…", false, None);
        let remote = Submenu::with_items(
            "원격",
            true,
            &[
                &status,
                &PredefinedMenuItem::separator(),
                &MenuItem::with_id(ID_CAD, "Ctrl+Alt+Del 보내기", true, None),
                &PredefinedMenuItem::separator(),
                &quality_menu,
                &display_menu,
                &PredefinedMenuItem::separator(),
                &swap_item,
                &caps_item,
                &clip_item,
            ],
        )?;
        menu.append_items(&[&app, &remote])?;
        Ok(Self {
            status,
            menu,
            swap: swap_item,
            clipboard: clip_item,
            caps_hangul: caps_item,
            qualities,
            displays: display_items,
        })
    }

    /// 화질 체크 표시를 하나만 남긴다.
    pub fn select_quality(&self, q: Quality) {
        for (qq, item) in &self.qualities {
            item.set_checked(*qq == q);
        }
    }

    pub fn set_display_open(&self, id: u32, open: bool) {
        if let Some((_, item)) = self.displays.iter().find(|(d, _)| *d == id) {
            item.set_checked(open);
        }
    }
}
