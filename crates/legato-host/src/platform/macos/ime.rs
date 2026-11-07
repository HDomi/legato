//! 한/영 입력 소스 전환 (Text Input Sources). 메인 스레드에서 불러야 한다.

use std::ffi::c_void;

use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::string::{CFString, CFStringRef};

type InputSourceRef = *const c_void;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> InputSourceRef;
    fn TISCreateInputSourceList(properties: CFDictionaryRef, include_all_installed: u8) -> CFArrayRef;
    fn TISSelectInputSource(source: InputSourceRef) -> i32;
    fn TISGetInputSourceProperty(source: InputSourceRef, key: CFStringRef) -> *const c_void;
    static kTISPropertyInputSourceID: CFStringRef;
    static kTISPropertyInputSourceCategory: CFStringRef;
    static kTISPropertyInputSourceIsSelectCapable: CFStringRef;
    static kTISCategoryKeyboardInputSource: CFStringRef;
}

fn source_id(src: InputSourceRef) -> String {
    unsafe {
        let p = TISGetInputSourceProperty(src, kTISPropertyInputSourceID);
        if p.is_null() { String::new() } else { CFString::wrap_under_get_rule(p as CFStringRef).to_string() }
    }
}

fn is_korean(id: &str) -> bool {
    id.contains("inputmethod.Korean")
}

/// 지금이 한국어면 영문 자판으로, 아니면 한국어 입력으로 바꾼다. 바꾼 뒤 입력 소스 ID 를 돌려준다.
pub fn toggle_korean() -> Option<String> {
    unsafe {
        let cur = TISCopyCurrentKeyboardInputSource();
        let cur_id = if cur.is_null() { String::new() } else { source_id(cur) };
        if !cur.is_null() {
            core_foundation::base::CFRelease(cur as _);
        }
        // 선택할 수 있는 키보드 입력 소스 목록.
        let filter = CFDictionary::from_CFType_pairs(&[
            (
                CFString::wrap_under_get_rule(kTISPropertyInputSourceCategory).as_CFType(),
                CFString::wrap_under_get_rule(kTISCategoryKeyboardInputSource).as_CFType(),
            ),
            (
                CFString::wrap_under_get_rule(kTISPropertyInputSourceIsSelectCapable).as_CFType(),
                CFBoolean::true_value().as_CFType(),
            ),
        ]);
        let list = TISCreateInputSourceList(filter.as_concrete_TypeRef(), 0);
        if list.is_null() {
            return None;
        }
        let list: CFArray<CFType> = CFArray::wrap_under_create_rule(list);
        let sources: Vec<(String, InputSourceRef)> =
            list.iter().map(|s| (source_id(s.as_CFTypeRef()), s.as_CFTypeRef())).collect();
        let want_korean = !is_korean(&cur_id);
        let pick = if want_korean {
            // 한국어 입력 모드(2벌식 등)를 고른다. 상위 입력기 항목보다 입력 모드를 우선.
            sources
                .iter()
                .find(|(id, _)| is_korean(id) && id.matches('.').count() >= 4)
                .or_else(|| sources.iter().find(|(id, _)| is_korean(id)))
        } else {
            // 영문 자판: ABC → US → 그 밖의 한국어가 아닌 자판.
            sources
                .iter()
                .find(|(id, _)| id == "com.apple.keylayout.ABC")
                .or_else(|| sources.iter().find(|(id, _)| id == "com.apple.keylayout.US"))
                .or_else(|| sources.iter().find(|(id, _)| id.starts_with("com.apple.keylayout.")))
        };
        let (id, src) = pick?;
        if TISSelectInputSource(*src) == 0 { Some(id.clone()) } else { None }
    }
}
