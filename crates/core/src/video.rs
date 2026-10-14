//! 直播画面的解码接口和追帧规则（设计文档第 6.5 节）。解码器的实现在 platform 里。

use crate::capture::Frame;

/// H.264 解码器。每次送进一帧 Annex B 格式的数据，返回解码出的最新一帧灰度画面；
/// 解码器还在攒数据时返回 None。返回的错误是给人看的原因。
pub trait H264Decoder {
    fn decode(&mut self, annexb: &[u8], timestamp_ms: u32) -> Result<Option<Frame>, String>;
}

/// 画面比最好的时候多落后这么多，就开始追帧。
pub const MAX_LAG_MS: i64 = 2_000;

/// 追帧（设计文档第 6.5 节）。
///
/// 用“收到的时间 - FLV 时间戳”衡量落后多少：这个差值越大，画面越旧。记下见过的最小差值
/// （离直播最近的时候），当前差值比它大出 [`MAX_LAG_MS`] 以上，说明网络卡过、积了一段旧画面，
/// 这时跳过非关键帧，等下一个关键帧再接着解。到了关键帧就以它为新的基准，
/// 免得网络一直慢时永远在跳帧。
#[derive(Debug, Default)]
pub struct CatchUp {
    best: Option<i64>,
    skipping: bool,
}

impl CatchUp {
    /// `received_ms` 是收到这一帧的时间（单调时钟，毫秒）。返回这一帧要不要解码。
    pub fn should_decode(&mut self, timestamp_ms: u32, key: bool, received_ms: u64) -> bool {
        let offset = received_ms as i64 - i64::from(timestamp_ms);
        let best = self.best.map_or(offset, |best| best.min(offset));
        self.best = Some(best);
        if self.skipping || offset - best > MAX_LAG_MS {
            if key {
                self.skipping = false;
                self.best = Some(offset);
                return true;
            }
            self.skipping = true;
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每 33 毫秒一帧，每 60 帧一个关键帧。
    fn key(index: u32) -> bool {
        index.is_multiple_of(60)
    }

    #[test]
    fn a_steady_stream_is_never_skipped() {
        let mut catch_up = CatchUp::default();
        for i in 0..300 {
            assert!(catch_up.should_decode(i * 33, key(i), u64::from(i * 33) + 500));
        }
    }

    #[test]
    fn a_burst_at_the_start_is_decoded() {
        // 刚连上时 CDN 一次补发 3 秒的画面，之后按正常速度来。
        let mut catch_up = CatchUp::default();
        for i in 0..90 {
            assert!(catch_up.should_decode(i * 33, key(i), u64::from(i)));
        }
        for i in 90..200 {
            assert!(catch_up.should_decode(i * 33, key(i), u64::from(i * 33) - 2_900));
        }
    }

    #[test]
    fn after_a_stall_it_skips_to_the_next_keyframe() {
        let mut catch_up = CatchUp::default();
        for i in 0..100 {
            assert!(catch_up.should_decode(i * 33, key(i), u64::from(i * 33)));
        }
        // 网络卡了 3 秒，之后的帧都晚到 3 秒。
        let late = |i: u32| u64::from(i * 33) + 3_000;
        assert!(!catch_up.should_decode(100 * 33, false, late(100)));
        for i in 101..120 {
            assert!(!catch_up.should_decode(i * 33, false, late(i)));
        }
        // 下一个关键帧解码，并以它为新的基准，之后照常解码。
        assert!(catch_up.should_decode(120 * 33, true, late(120)));
        for i in 121..180 {
            assert!(catch_up.should_decode(i * 33, key(i), late(i)), "{i}");
        }
    }

    #[test]
    fn a_late_keyframe_is_decoded_right_away() {
        let mut catch_up = CatchUp::default();
        assert!(catch_up.should_decode(0, true, 0));
        assert!(catch_up.should_decode(33, true, 5_033));
        assert!(catch_up.should_decode(66, false, 5_066));
    }
}
