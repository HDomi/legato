//! 뷰어 창들. 원격 디스플레이마다 창 하나. 원격 화면을 그리고 로컬 입력을 호스트로 보낸다.
//! macOS 에서는 기본으로 네이티브 전체화면(별도 Space)으로 열고, 로컬 모니터가 여러 대면 하나씩 나눠 띄운다.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use legato_core::codec::I420Frame;
use legato_core::keymap::swap_cmd_ctrl;
use legato_core::protocol::{ClientMsg, DisplayInfo, InputEvent, MouseButton, Quality};
use tokio::sync::mpsc::UnboundedSender;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};
use winit::window::{Fullscreen, Window, WindowId};

use crate::keys::to_w3c;
use crate::menu::{self, ViewerMenu};
use crate::render::{Renderer, Viewport};
use crate::stream::{self, Routes};

/// 네트워크·디코딩·메뉴 쪽에서 창으로 보내는 알림.
#[derive(Debug)]
pub enum UserEvent {
    NewFrame(u32),
    Notice(String),
    Closed(String),
    Menu(String),
    QualityChanged(u32, Quality),
}

/// 원격 디스플레이 하나를 보여주는 창.
struct ViewWin {
    display_id: u32,
    window: Arc<Window>,
    renderer: Renderer,
    viewport: Option<Viewport>,
    latest: Arc<Mutex<Option<I420Frame>>>,
    cursor_inside: bool,
}

/// 뷰어 설정과 세션 정보.
pub struct AppConfig {
    pub host_name: String,
    pub fullscreen: bool,
    pub displays: Vec<DisplayInfo>,
    /// 처음에 열 디스플레이.
    pub open: Vec<u32>,
    pub quality: Quality,
    /// Cmd↔Ctrl 교환 (OS 가 다를 때 기본 켜짐).
    pub swap: bool,
    /// 클립보드 공유 여부 (메뉴에서 끄고 켬).
    pub clipboard_on: Arc<AtomicBool>,
    /// Caps Lock 을 원격 한/영 전환 키로 쓴다 (macOS 에서 Caps Lock 으로 한/영을 바꾸는 사람용).
    pub caps_hangul: bool,
}

pub struct App {
    cfg: AppConfig,
    ctl: UnboundedSender<ClientMsg>,
    routes: Routes,
    proxy: EventLoopProxy<UserEvent>,
    menu: Option<ViewerMenu>,
    wins: HashMap<WindowId, ViewWin>,
    started: bool,
    /// Caps Lock 마지막 이벤트 (눌림 여부, 시각). 한 번 누를 때 눌림·뗌이 연달아 오는지 가려낸다.
    last_caps: Option<(bool, std::time::Instant)>,
    pub exit_message: Option<String>,
}

impl App {
    pub fn new(
        cfg: AppConfig,
        ctl: UnboundedSender<ClientMsg>,
        routes: Routes,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Self {
        Self {
            cfg,
            ctl,
            routes,
            proxy,
            menu: None,
            wins: HashMap::new(),
            started: false,
            last_caps: None,
            exit_message: None,
        }
    }

    fn send_input(&self, display_id: u32, event: InputEvent) {
        let _ = self.ctl.send(ClientMsg::Input { display_id, event });
    }

    fn title(&self, d: &DisplayInfo) -> String {
        if self.cfg.displays.len() > 1 {
            format!("Legato — {} · {}", self.cfg.host_name, d.name)
        } else {
            format!("Legato — {}", self.cfg.host_name)
        }
    }

    /// 디스플레이 창을 열고 영상 스트림을 요청한다. `slot` 번째 로컬 모니터에 둔다.
    fn open_display(&mut self, event_loop: &ActiveEventLoop, display_id: u32, slot: usize) {
        if self.wins.values().any(|w| w.display_id == display_id) {
            return;
        }
        let Some(info) = self.cfg.displays.iter().find(|d| d.id == display_id).cloned() else { return };
        let monitors: Vec<_> = event_loop.available_monitors().collect();
        let monitor = if monitors.is_empty() { None } else { Some(monitors[slot % monitors.len()].clone()) };

        // 원격 비율을 유지하면서 화면에 들어갈 정도 크기로 연다.
        let (rw, rh) = (info.width.max(1) as f64, info.height.max(1) as f64);
        let scale = (1280.0 / rw).min(800.0 / rh).min(1.0);
        let mut attrs = Window::default_attributes()
            .with_title(self.title(&info))
            .with_inner_size(LogicalSize::new(rw * scale, rh * scale));
        if let Some(m) = &monitor {
            // 창 모드에서도 디스플레이마다 다른 로컬 모니터에 둔다.
            let p = m.position();
            attrs = attrs.with_position(PhysicalPosition::new(p.x + 80, p.y + 80));
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.exit_message = Some(format!("창 생성 실패: {e}"));
                event_loop.exit();
                return;
            }
        };
        let renderer = match Renderer::new(window.clone()) {
            Ok(r) => r,
            Err(e) => {
                self.exit_message = Some(format!("렌더러 생성 실패: {e:#}"));
                event_loop.exit();
                return;
            }
        };
        #[cfg(windows)]
        if let Some(m) = &self.menu {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = window.window_handle()
                && let RawWindowHandle::Win32(h) = h.as_raw()
            {
                // Windows 는 창마다 메뉴 막대를 붙인다.
                let _ = unsafe { m.menu.init_for_hwnd(h.hwnd.get()) };
            }
        }
        if self.cfg.fullscreen {
            // macOS: Borderless 전체화면은 toggleFullScreen 을 써서 Mission Control 에 별도 Space 로 생긴다.
            window.set_fullscreen(Some(Fullscreen::Borderless(monitor)));
        }

        // 디코딩 스레드와 라우팅.
        let latest: Arc<Mutex<Option<I420Frame>>> = Arc::new(Mutex::new(None));
        let input = {
            let latest = latest.clone();
            let proxy = self.proxy.clone();
            stream::spawn_decoder(display_id, self.ctl.clone(), move |frame, _capture_us| {
                *latest.lock().expect("뮤텍스") = Some(frame);
                let _ = proxy.send_event(UserEvent::NewFrame(display_id));
            })
        };
        self.routes.lock().expect("뮤텍스").insert(display_id, input);
        let _ = self.ctl.send(ClientMsg::StartStream { display_id, quality: self.cfg.quality });
        window.request_redraw();
        if let Some(m) = &self.menu {
            m.set_display_open(display_id, true);
        }
        self.wins.insert(
            window.id(),
            ViewWin { display_id, window, renderer, viewport: None, latest, cursor_inside: false },
        );
    }

    /// 창을 닫고 스트림을 멈춘다. 마지막 창이면 연결을 끝낸다.
    fn close_window(&mut self, event_loop: &ActiveEventLoop, id: WindowId) {
        let Some(w) = self.wins.remove(&id) else { return };
        self.routes.lock().expect("뮤텍스").remove(&w.display_id);
        let _ = self.ctl.send(ClientMsg::StopStream { display_id: w.display_id });
        self.send_input(w.display_id, InputEvent::ReleaseAll);
        if let Some(m) = &self.menu {
            m.set_display_open(w.display_id, false);
        }
        if self.wins.is_empty() {
            event_loop.exit();
        }
    }

    fn handle_menu(&mut self, event_loop: &ActiveEventLoop, id: &str) {
        match id {
            menu::ID_DISCONNECT => event_loop.exit(),
            menu::ID_CAD => {
                if let Some(d) = self.wins.values().next().map(|w| w.display_id) {
                    self.send_input(d, InputEvent::SecureAttention);
                }
            }
            menu::ID_SWAP => {
                self.cfg.swap = !self.cfg.swap;
                if let Some(m) = &self.menu {
                    m.swap.set_checked(self.cfg.swap);
                }
            }
            menu::ID_CAPS_HANGUL => {
                self.cfg.caps_hangul = !self.cfg.caps_hangul;
                if let Some(m) = &self.menu {
                    m.caps_hangul.set_checked(self.cfg.caps_hangul);
                }
            }
            menu::ID_CLIPBOARD => {
                let on = !self.cfg.clipboard_on.load(Ordering::SeqCst);
                self.cfg.clipboard_on.store(on, Ordering::SeqCst);
                if let Some(m) = &self.menu {
                    m.clipboard.set_checked(on);
                }
            }
            other => {
                if let Some(q) = other.strip_prefix(menu::ID_QUALITY_PREFIX) {
                    let Some(q) = menu::QUALITIES.into_iter().find(|x| format!("{x:?}") == q) else { return };
                    self.cfg.quality = q;
                    if let Some(m) = &self.menu {
                        m.select_quality(q);
                    }
                    for w in self.wins.values() {
                        let _ = self.ctl.send(ClientMsg::StartStream { display_id: w.display_id, quality: q });
                    }
                } else if let Some(d) = other.strip_prefix(menu::ID_DISPLAY_PREFIX) {
                    let Ok(d) = d.parse::<u32>() else { return };
                    if let Some(wid) = self.wins.iter().find(|(_, w)| w.display_id == d).map(|(id, _)| *id) {
                        if self.wins.len() > 1 {
                            self.close_window(event_loop, wid);
                        } else if let Some(m) = &self.menu {
                            // 마지막 창은 메뉴로 닫지 않는다 (연결 끊기는 따로 있음).
                            m.set_display_open(d, true);
                        }
                    } else {
                        let slot = self.wins.len();
                        self.open_display(event_loop, d, slot);
                    }
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            return;
        }
        self.started = true;
        match ViewerMenu::new(
            &self.cfg.host_name,
            self.cfg.quality,
            self.cfg.swap,
            self.cfg.caps_hangul,
            &self.cfg.displays,
            &self.cfg.open,
        ) {
            Ok(m) => {
                #[cfg(target_os = "macos")]
                m.menu.init_for_nsapp();
                let proxy = self.proxy.clone();
                muda::MenuEvent::set_event_handler(Some(move |e: muda::MenuEvent| {
                    let _ = proxy.send_event(UserEvent::Menu(e.id.0.clone()));
                }));
                self.menu = Some(m);
            }
            Err(e) => tracing::warn!("메뉴 생성 실패: {e}"),
        }
        let open = self.cfg.open.clone();
        for (slot, id) in open.into_iter().enumerate() {
            self.open_display(event_loop, id, slot);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::NewFrame(d) => {
                if let Some(w) = self.wins.values().find(|w| w.display_id == d) {
                    w.window.request_redraw();
                }
            }
            UserEvent::Notice(msg) => {
                for w in self.wins.values() {
                    let base = w.window.title();
                    let base = base.split(" — 알림:").next().unwrap_or(&base).to_string();
                    w.window.set_title(&format!("{base} — 알림: {msg}"));
                }
            }
            UserEvent::QualityChanged(d, q) => {
                tracing::info!("디스플레이 {d} 자동 화질 → {}", q.label());
            }
            UserEvent::Menu(id) => self.handle_menu(event_loop, &id),
            UserEvent::Closed(msg) => {
                self.exit_message = Some(msg);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::CloseRequested) {
            self.close_window(event_loop, id);
            return;
        }
        let swap = self.cfg.swap;
        let caps_hangul = self.cfg.caps_hangul;
        // Caps Lock 처리에서 self 를 빌려야 해서 창 정보는 필요한 것만 먼저 꺼낸다.
        if let WindowEvent::KeyboardInput { .. } = &event
            && let Some(display_id) = self.wins.get(&id).map(|w| w.display_id)
        {
            if let WindowEvent::KeyboardInput { event: ke, .. } = &event
                && let Some(code) = to_w3c(ke.physical_key)
            {
                let down = ke.state == ElementState::Pressed;
                if code == "CapsLock" && caps_hangul {
                    let now = std::time::Instant::now();
                    let same_press =
                        matches!(self.last_caps, Some((true, t)) if !down && now.duration_since(t).as_millis() < 500);
                    self.last_caps = Some((down, now));
                    if !same_press {
                        for d in [true, false] {
                            self.send_input(display_id, InputEvent::Key { code: "Lang1".into(), down: d });
                        }
                    }
                    return;
                }
                let code = if swap { swap_cmd_ctrl(&code).to_string() } else { code };
                self.send_input(display_id, InputEvent::Key { code, down });
            }
            return;
        }
        let Some(w) = self.wins.get_mut(&id) else { return };
        let display_id = w.display_id;
        let mut input: Option<InputEvent> = None;
        match event {
            WindowEvent::Resized(size) => {
                w.renderer.resize(size.width, size.height);
                w.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                if let Some(frame) = w.latest.lock().expect("뮤텍스").take() {
                    w.renderer.upload(&frame);
                }
                if let Some(vp) = w.renderer.render() {
                    w.viewport = Some(vp);
                }
            }
            WindowEvent::Focused(false) => input = Some(InputEvent::ReleaseAll),
            WindowEvent::CursorMoved { position, .. } => {
                let Some(vp) = w.viewport else { return };
                match vp.normalize(position.x, position.y) {
                    Some((x, y)) => {
                        w.cursor_inside = true;
                        input = Some(InputEvent::MouseMove { x, y });
                    }
                    None => w.cursor_inside = false,
                }
            }
            WindowEvent::CursorLeft { .. } => w.cursor_inside = false,
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    winit::event::MouseButton::Back => MouseButton::Back,
                    winit::event::MouseButton::Forward => MouseButton::Forward,
                    winit::event::MouseButton::Other(_) => return,
                };
                let down = state == ElementState::Pressed;
                // 누르기는 화면 안에서만, 떼기는 항상 보낸다 (드래그가 밖에서 끝나도 버튼이 남지 않게).
                if down && !w.cursor_inside {
                    return;
                }
                input = Some(InputEvent::MouseButton { button, down });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !w.cursor_inside {
                    return;
                }
                input = Some(match delta {
                    MouseScrollDelta::LineDelta(x, y) => InputEvent::Scroll { dx: x, dy: y, pixels: false },
                    MouseScrollDelta::PixelDelta(p) => {
                        let l = p.to_logical::<f64>(w.window.scale_factor());
                        InputEvent::Scroll { dx: l.x as f32, dy: l.y as f32, pixels: true }
                    }
                });
            }
            _ => {}
        }
        if let Some(ev) = input {
            self.send_input(display_id, ev);
        }
    }
}
