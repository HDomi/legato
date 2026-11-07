//! 뷰어 창. 원격 화면을 그리고 로컬 입력을 호스트로 보낸다.
//! macOS 에서는 기본으로 네이티브 전체화면(별도 Space)으로 열린다.

use std::sync::{Arc, Mutex};

use legato_core::codec::I420Frame;
use legato_core::protocol::{ClientMsg, InputEvent, MouseButton};
use tokio::sync::mpsc::UnboundedSender;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Fullscreen, Window, WindowId};

use crate::keys::to_w3c;
use crate::render::{Renderer, Viewport};

/// 네트워크·디코딩 쪽에서 창으로 보내는 알림.
#[derive(Debug)]
pub enum UserEvent {
    NewFrame,
    Notice(String),
    Closed(String),
}

pub struct App {
    pub title: String,
    pub fullscreen: bool,
    /// 처음 창 크기를 정할 원격 화면 크기 (포인트).
    pub initial_size: (u32, u32),
    pub latest: Arc<Mutex<Option<I420Frame>>>,
    pub ctl: UnboundedSender<ClientMsg>,
    pub exit_message: Option<String>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    viewport: Option<Viewport>,
    cursor_inside: bool,
}

impl App {
    pub fn new(
        title: String,
        fullscreen: bool,
        initial_size: (u32, u32),
        latest: Arc<Mutex<Option<I420Frame>>>,
        ctl: UnboundedSender<ClientMsg>,
    ) -> Self {
        Self {
            title,
            fullscreen,
            initial_size,
            latest,
            ctl,
            exit_message: None,
            window: None,
            renderer: None,
            viewport: None,
            cursor_inside: false,
        }
    }

    fn send_input(&self, ev: InputEvent) {
        let _ = self.ctl.send(ClientMsg::Input(ev));
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // 원격 비율을 유지하면서 화면에 들어갈 정도 크기로 연다.
        let (rw, rh) = (self.initial_size.0.max(1) as f64, self.initial_size.1.max(1) as f64);
        let scale = (1280.0 / rw).min(800.0 / rh).min(1.0);
        let attrs = Window::default_attributes()
            .with_title(&self.title)
            .with_inner_size(LogicalSize::new(rw * scale, rh * scale));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.exit_message = Some(format!("창 생성 실패: {e}"));
                event_loop.exit();
                return;
            }
        };
        match Renderer::new(window.clone()) {
            Ok(r) => self.renderer = Some(r),
            Err(e) => {
                self.exit_message = Some(format!("렌더러 생성 실패: {e:#}"));
                event_loop.exit();
                return;
            }
        }
        if self.fullscreen {
            // macOS: Borderless 전체화면은 toggleFullScreen 을 써서 Mission Control 에 별도 Space 로 생긴다.
            window.set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        window.request_redraw();
        self.window = Some(window);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::NewFrame => {
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            UserEvent::Notice(msg) => {
                if let Some(w) = &self.window {
                    w.set_title(&format!("{} — {msg}", self.title));
                }
            }
            UserEvent::Closed(msg) => {
                self.exit_message = Some(msg);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(r) = self.renderer.as_mut() {
                    r.resize(size.width, size.height);
                }
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let Some(r) = self.renderer.as_mut() else { return };
                if let Some(frame) = self.latest.lock().expect("뮤텍스").take() {
                    r.upload(&frame);
                }
                if let Some(vp) = r.render() {
                    self.viewport = Some(vp);
                }
            }
            WindowEvent::Focused(false) => self.send_input(InputEvent::ReleaseAll),
            WindowEvent::CursorMoved { position, .. } => {
                let Some(vp) = self.viewport else { return };
                match vp.normalize(position.x, position.y) {
                    Some((x, y)) => {
                        self.cursor_inside = true;
                        self.send_input(InputEvent::MouseMove { x, y });
                    }
                    None => self.cursor_inside = false,
                }
            }
            WindowEvent::CursorLeft { .. } => self.cursor_inside = false,
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
                if down && !self.cursor_inside {
                    return;
                }
                self.send_input(InputEvent::MouseButton { button, down });
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if !self.cursor_inside {
                    return;
                }
                let ev = match delta {
                    MouseScrollDelta::LineDelta(x, y) => InputEvent::Scroll { dx: x, dy: y, pixels: false },
                    MouseScrollDelta::PixelDelta(p) => {
                        let scale = self.window.as_ref().map(|w| w.scale_factor()).unwrap_or(1.0);
                        let l = p.to_logical::<f64>(scale);
                        InputEvent::Scroll { dx: l.x as f32, dy: l.y as f32, pixels: true }
                    }
                };
                self.send_input(ev);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(code) = to_w3c(event.physical_key) {
                    self.send_input(InputEvent::Key { code, down: event.state == ElementState::Pressed });
                }
            }
            _ => {}
        }
    }
}
