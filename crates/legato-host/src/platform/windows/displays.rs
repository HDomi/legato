//! DXGI 출력(모니터) 목록. 디스플레이 번호는 어댑터·출력 순서로 정한다 (어댑터×100 + 출력 + 1).

use anyhow::{Context, Result};
use legato_core::protocol::DisplayInfo;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput};

pub struct Output {
    pub id: u32,
    pub adapter: IDXGIAdapter1,
    pub output: IDXGIOutput,
    /// 가상 데스크톱 좌표 (물리 픽셀).
    pub rect: RECT,
}

/// 데스크톱에 붙은 모든 출력.
pub fn outputs() -> Result<Vec<Output>> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.context("DXGI 팩토리 생성 실패")?;
    let mut list = Vec::new();
    let mut ai = 0u32;
    while let Ok(adapter) = unsafe { factory.EnumAdapters1(ai) } {
        let mut oi = 0u32;
        while let Ok(output) = unsafe { adapter.EnumOutputs(oi) } {
            if let Ok(desc) = unsafe { output.GetDesc() }
                && desc.AttachedToDesktop.as_bool()
            {
                list.push(Output {
                    id: ai * 100 + oi + 1,
                    adapter: adapter.clone(),
                    output,
                    rect: desc.DesktopCoordinates,
                });
            }
            oi += 1;
        }
        ai += 1;
    }
    Ok(list)
}

pub fn find(id: u32) -> Result<Output> {
    let all = outputs()?;
    let primary = all.iter().position(|o| o.rect.left == 0 && o.rect.top == 0);
    let idx = all.iter().position(|o| o.id == id).or(primary).or(if all.is_empty() { None } else { Some(0) });
    idx.map(|i| all.into_iter().nth(i).expect("인덱스 확인함")).context("캡처할 디스플레이가 없음")
}

pub fn list_displays() -> Result<Vec<DisplayInfo>> {
    Ok(outputs()?
        .into_iter()
        .enumerate()
        .map(|(i, o)| DisplayInfo {
            id: o.id,
            name: format!("디스플레이 {}", i + 1),
            width: (o.rect.right - o.rect.left).max(0) as u32,
            height: (o.rect.bottom - o.rect.top).max(0) as u32,
            primary: o.rect.left == 0 && o.rect.top == 0,
        })
        .collect())
}
