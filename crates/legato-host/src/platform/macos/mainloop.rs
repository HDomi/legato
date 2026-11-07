//! 메인 스레드 실행. 입력 소스 전환(TIS) 같은 API 는 메인 스레드에서 불러야 안전하다.
//! 호스트는 메인 스레드에서 CFRunLoop 를 돌리고, 서버(tokio)는 다른 스레드에서 돈다.

use std::ffi::c_void;
use std::sync::mpsc;

use anyhow::Result;
use core_foundation::date::CFAbsoluteTimeGetCurrent;
use core_foundation::runloop::{CFRunLoop, CFRunLoopTimer, kCFRunLoopDefaultMode};

unsafe extern "C" {
    static _dispatch_main_q: c_void;
    fn dispatch_async_f(queue: *const c_void, context: *mut c_void, work: extern "C" fn(*mut c_void));
}

extern "C" fn trampoline(ctx: *mut c_void) {
    let f: Box<Box<dyn FnOnce() + Send>> = unsafe { Box::from_raw(ctx as *mut Box<dyn FnOnce() + Send>) };
    f();
}

/// 메인 스레드에서 실행하도록 넘긴다 (결과를 기다리지 않음).
pub fn run_on_main(f: impl FnOnce() + Send + 'static) {
    let boxed: Box<Box<dyn FnOnce() + Send>> = Box::new(Box::new(f));
    unsafe { dispatch_async_f(&_dispatch_main_q, Box::into_raw(boxed) as *mut c_void, trampoline) };
}

extern "C" fn keep_alive(_timer: core_foundation::runloop::CFRunLoopTimerRef, _info: *mut c_void) {}

/// `work` 를 별도 스레드에서 돌리고, 끝날 때까지 메인 스레드는 run loop 를 돌며 메인 큐 작업을 처리한다.
pub fn run_with_main_loop(work: impl FnOnce() -> Result<()> + Send + 'static) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let main = CFRunLoop::get_current();
    std::thread::Builder::new().name("legato-server".into()).spawn(move || {
        let r = work();
        let _ = tx.send(r);
        // 메인 run loop 를 깨워 끝나게 한다.
        run_on_main(|| CFRunLoop::get_current().stop());
    })?;
    // 소스가 없으면 run loop 가 바로 끝나므로 1초짜리 빈 타이머를 둔다.
    let timer =
        CFRunLoopTimer::new(unsafe { CFAbsoluteTimeGetCurrent() } + 1.0, 1.0, 0, 0, keep_alive, std::ptr::null_mut());
    main.add_timer(&timer, unsafe { kCFRunLoopDefaultMode });
    loop {
        CFRunLoop::run_current();
        if let Ok(r) = rx.try_recv() {
            return r;
        }
    }
}
