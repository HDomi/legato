// ScreenCaptureKit 바인딩이 Swift 런타임을 링크한다. 의존 크레이트의 rpath 설정은
// 최종 실행 파일까지 전달되지 않으므로 여기서 직접 넣는다.
fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,/usr/lib/swift");
    }
}
