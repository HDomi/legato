//! 자동 화질 조절. 호스트가 1초마다 혼잡 신호를 넣으면 단계를 올리거나 내린다.
//!
//! 혼잡 신호: 인코더가 네트워크를 기다리느라 버린 캡처 프레임 비율, 전송 대기 시간.
//! 내릴 때는 빠르게(2초 연속 혼잡), 올릴 때는 천천히(10초 연속 여유) 움직여 출렁임을 막는다.
//! 내린 뒤에는 한동안(30초부터, 반복될 때마다 두 배, 최대 10분) 다시 올리지 않는다.
//! 높은 단계를 감당 못 하는 환경에서 올렸다 내렸다를 반복하지 않게.

use crate::protocol::Quality;

/// 1초 동안의 측정값.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    /// 캡처는 됐는데 인코더가 바빠 버린 프레임 수.
    pub dropped: u32,
    /// 보낸 프레임 수.
    pub sent: u32,
    /// 프레임 하나를 네트워크에 넘기기까지 기다린 최대 시간 (ms).
    pub max_send_wait_ms: u32,
    /// 뷰어가 디코딩이 밀려 키프레임을 다시 요청한 횟수 (뷰어 쪽 처리 한계 신호).
    pub keyframe_requests: u32,
}

impl Sample {
    fn congested(&self) -> bool {
        let total = self.dropped + self.sent;
        let drop_ratio = if total == 0 { 0.0 } else { self.dropped as f32 / total as f32 };
        drop_ratio > 0.3 || self.max_send_wait_ms > 150 || self.keyframe_requests > 0
    }

    fn healthy(&self) -> bool {
        self.dropped == 0 && self.max_send_wait_ms < 30 && self.keyframe_requests == 0
    }
}

/// 단계 조절기. `Quality::Auto` 일 때만 쓴다.
pub struct AdaptiveController {
    level: Quality,
    bad_streak: u32,
    good_streak: u32,
    /// 남은 올리기 금지 시간 (초).
    up_block: u32,
    /// 다음에 내릴 때 걸 올리기 금지 시간 (초).
    penalty: u32,
}

const FIRST_PENALTY: u32 = 30;
const MAX_PENALTY: u32 = 600;

impl Default for AdaptiveController {
    fn default() -> Self {
        Self { level: Quality::Balanced, bad_streak: 0, good_streak: 0, up_block: 0, penalty: FIRST_PENALTY }
    }
}

impl AdaptiveController {
    pub fn level(&self) -> Quality {
        self.level
    }

    /// 1초 측정값을 넣는다. 단계가 바뀌면 새 단계를 돌려준다.
    pub fn push(&mut self, s: Sample) -> Option<Quality> {
        self.up_block = self.up_block.saturating_sub(1);
        if s.congested() {
            self.bad_streak += 1;
            self.good_streak = 0;
        } else if s.healthy() {
            self.good_streak += 1;
            self.bad_streak = 0;
        } else {
            self.bad_streak = 0;
            self.good_streak = 0;
        }
        let next = if self.bad_streak >= 2 {
            match self.level {
                Quality::High => Some(Quality::Balanced),
                Quality::Balanced => Some(Quality::Low),
                _ => None,
            }
        } else if self.good_streak >= 10 && self.up_block == 0 {
            match self.level {
                Quality::Low => Some(Quality::Balanced),
                Quality::Balanced => Some(Quality::High),
                _ => None,
            }
        } else {
            None
        };
        if let Some(n) = next {
            if self.bad_streak >= 2 {
                // 내렸으면 한동안 올리지 않고, 다음 금지 시간은 두 배로.
                self.up_block = self.penalty;
                self.penalty = (self.penalty * 2).min(MAX_PENALTY);
            }
            self.level = n;
            self.bad_streak = 0;
            self.good_streak = 0;
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 혼잡하면_내리고_여유있으면_올린다() {
        let mut c = AdaptiveController::default();
        let bad = Sample { dropped: 20, sent: 10, max_send_wait_ms: 300, keyframe_requests: 0 };
        let good = Sample { dropped: 0, sent: 30, max_send_wait_ms: 5, keyframe_requests: 0 };
        // 처음에는 10초 여유면 올라간다.
        for _ in 0..9 {
            assert_eq!(c.push(good), None);
        }
        assert_eq!(c.push(good), Some(Quality::High));
        // 2초 혼잡이면 내려간다.
        assert_eq!(c.push(bad), None);
        assert_eq!(c.push(bad), Some(Quality::Balanced));
        // 내린 직후 30초 동안은 여유가 있어도 안 올린다.
        for _ in 0..29 {
            assert_eq!(c.push(good), None);
        }
        assert_eq!(c.push(good), Some(Quality::High));
        // 다시 내려가면 금지 시간이 60초로 늘어난다.
        c.push(bad);
        assert_eq!(c.push(bad), Some(Quality::Balanced));
        for _ in 0..59 {
            assert_eq!(c.push(good), None);
        }
        assert_eq!(c.push(good), Some(Quality::High));
        // 가장 낮은 단계에서는 더 안 내린다.
        let mut c = AdaptiveController::default();
        c.push(bad);
        assert_eq!(c.push(bad), Some(Quality::Low));
        c.push(bad);
        assert_eq!(c.push(bad), None);
        // 중간 상태는 연속 기록을 끊는다.
        let meh = Sample { dropped: 1, sent: 29, max_send_wait_ms: 40, keyframe_requests: 0 };
        let mut c = AdaptiveController::default();
        c.push(bad);
        c.push(meh);
        assert_eq!(c.push(bad), None);
        // 뷰어 키프레임 요청은 혼잡으로 본다.
        assert!(Sample { keyframe_requests: 1, sent: 30, ..Default::default() }.congested());
    }
}
