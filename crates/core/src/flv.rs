//! FLV 拆包（设计文档第 6.5 节）：从直播流里取出 H.264 视频帧，转成解码器要的 Annex B 格式。
//! 边收边拆：每收到一块数据就 `push` 进来，取出已经完整的帧；不完整的部分留到下次。

/// 一帧 H.264 视频，Annex B 格式：每个 NAL 前面是起始码 `00 00 00 01`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    pub annexb: Vec<u8>,
    /// 关键帧。前面已经补上 SPS/PPS，可以从这一帧开始解码。
    pub key: bool,
    /// FLV 时间戳，毫秒。
    pub timestamp_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlvError {
    #[error("直播流不是 FLV 格式")]
    NotFlv,
    #[error("直播流不是 H.264")]
    NotH264,
    #[error("直播流的 H.264 配置不对")]
    BadConfig,
}

const START_CODE: [u8; 4] = [0, 0, 0, 1];
/// 标签头：类型 1 字节、数据长度 3 字节、时间戳 3+1 字节、流 ID 3 字节。
const TAG_HEADER: usize = 11;
/// 标签后面跟 4 字节的 PreviousTagSize。
const TAG_TRAILER: usize = 4;
const TAG_VIDEO: u8 = 9;
const MAX_HEADER: usize = 1024;
const CODEC_AVC: u8 = 7;
const FRAME_KEY: u8 = 1;

#[derive(Debug, Default)]
pub struct FlvParser {
    buffer: Vec<u8>,
    header_read: bool,
    avc: Avc,
}

/// 从 H.264 配置里读到的东西。
#[derive(Debug, Default)]
struct Avc {
    /// NAL 长度前缀占几个字节。0 表示还没收到配置。
    nal_length_size: usize,
    /// Annex B 格式的 SPS 和 PPS，关键帧前要补上。
    parameter_sets: Vec<u8>,
}

impl FlvParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 收到一块数据，返回已经完整的视频帧。音频和脚本标签直接跳过；
    /// 还没收到 H.264 配置之前的视频帧也跳过，因为解不了。
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<VideoFrame>, FlvError> {
        self.buffer.extend_from_slice(bytes);
        let mut frames = Vec::new();
        let mut pos = 0;
        if !self.header_read {
            // 文件头：“FLV”、版本、标志、头长度（4 字节），后面跟 4 字节的 PreviousTagSize0。
            if self.buffer.len() >= 3 && &self.buffer[..3] != b"FLV" {
                return Err(FlvError::NotFlv);
            }
            if self.buffer.len() < 9 {
                return Ok(frames);
            }
            let header_len = u32::from_be_bytes([
                self.buffer[5],
                self.buffer[6],
                self.buffer[7],
                self.buffer[8],
            ]) as usize;
            // 文件头正常是 9 字节。长得离谱就不是 FLV，免得一直攒数据等它。
            if !(9..=MAX_HEADER).contains(&header_len) {
                return Err(FlvError::NotFlv);
            }
            if self.buffer.len() < header_len + TAG_TRAILER {
                return Ok(frames);
            }
            pos = header_len + TAG_TRAILER;
            self.header_read = true;
        }
        while self.buffer.len() >= pos + TAG_HEADER {
            let tag = &self.buffer[pos..];
            let size = u32::from_be_bytes([0, tag[1], tag[2], tag[3]]) as usize;
            let end = pos + TAG_HEADER + size + TAG_TRAILER;
            if self.buffer.len() < end {
                break;
            }
            // 时间戳：低 24 位在前，第 4 个字节是最高 8 位。
            let timestamp_ms = u32::from_be_bytes([tag[7], tag[4], tag[5], tag[6]]);
            if tag[0] & 0x1f == TAG_VIDEO {
                let body = &self.buffer[pos + TAG_HEADER..pos + TAG_HEADER + size];
                if let Some(frame) = self.avc.video_tag(body, timestamp_ms)? {
                    frames.push(frame);
                }
            }
            pos = end;
        }
        self.buffer.drain(..pos);
        Ok(frames)
    }
}

impl Avc {
    fn video_tag(
        &mut self,
        body: &[u8],
        timestamp_ms: u32,
    ) -> Result<Option<VideoFrame>, FlvError> {
        let Some(&first) = body.first() else {
            return Ok(None);
        };
        // 增强 FLV（H.265 等）把最高位置 1；传统 FLV 的低 4 位是编码 ID，7 是 H.264。
        if first & 0x80 != 0 || first & 0x0f != CODEC_AVC {
            return Err(FlvError::NotH264);
        }
        // 第 2 个字节是包的类型，后面 3 个字节是合成时间偏移，用不上。
        if body.len() < 5 {
            return Ok(None);
        }
        match body[1] {
            0 => {
                self.read_config(&body[5..])?;
                Ok(None)
            }
            1 => Ok(self.read_nalus(&body[5..], first >> 4 == FRAME_KEY, timestamp_ms)),
            // 2 是序列结束。
            _ => Ok(None),
        }
    }

    /// 读 AVCDecoderConfigurationRecord：NAL 长度前缀的字节数、SPS 和 PPS。
    fn read_config(&mut self, record: &[u8]) -> Result<(), FlvError> {
        if record.len() < 6 {
            return Err(FlvError::BadConfig);
        }
        let mut sets = Vec::new();
        let mut pos = 6;
        for _ in 0..(record[5] & 0x1f) {
            pos = copy_parameter_set(record, pos, &mut sets)?;
        }
        let pps_count = *record.get(pos).ok_or(FlvError::BadConfig)?;
        pos += 1;
        for _ in 0..pps_count {
            pos = copy_parameter_set(record, pos, &mut sets)?;
        }
        self.nal_length_size = (record[4] & 0x03) as usize + 1;
        self.parameter_sets = sets;
        Ok(())
    }

    /// 把“长度 + NAL”的序列改写成“起始码 + NAL”。关键帧前补上 SPS/PPS。
    fn read_nalus(&self, data: &[u8], key: bool, timestamp_ms: u32) -> Option<VideoFrame> {
        if self.nal_length_size == 0 {
            return None;
        }
        let mut annexb = Vec::with_capacity(data.len() + self.parameter_sets.len() + 16);
        if key {
            annexb.extend_from_slice(&self.parameter_sets);
        }
        let mut pos = 0;
        while pos + self.nal_length_size <= data.len() {
            let len = data[pos..pos + self.nal_length_size]
                .iter()
                .fold(0usize, |n, byte| (n << 8) | *byte as usize);
            pos += self.nal_length_size;
            let Some(nal) = data.get(pos..pos.saturating_add(len)) else {
                break;
            };
            annexb.extend_from_slice(&START_CODE);
            annexb.extend_from_slice(nal);
            pos += len;
        }
        Some(VideoFrame {
            annexb,
            key,
            timestamp_ms,
        })
    }
}

/// 复制一个“2 字节长度 + 内容”的参数集，前面加起始码。返回下一个参数集的位置。
fn copy_parameter_set(record: &[u8], pos: usize, out: &mut Vec<u8>) -> Result<usize, FlvError> {
    let len = record.get(pos..pos + 2).ok_or(FlvError::BadConfig)?;
    let len = u16::from_be_bytes([len[0], len[1]]) as usize;
    let set = record
        .get(pos + 2..pos + 2 + len)
        .ok_or(FlvError::BadConfig)?;
    out.extend_from_slice(&START_CODE);
    out.extend_from_slice(set);
    Ok(pos + 2 + len)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: [u8; 4] = [0x67, 0x64, 0x00, 0x1f];
    const PPS: [u8; 4] = [0x68, 0xee, 0x3c, 0x80];
    const IDR: [u8; 3] = [0x65, 0x88, 0x84];
    const SLICE: [u8; 3] = [0x41, 0x9a, 0x02];

    fn header() -> Vec<u8> {
        b"FLV\x01\x05\x00\x00\x00\x09\x00\x00\x00\x00".to_vec()
    }

    fn tag(kind: u8, timestamp: u32, body: &[u8]) -> Vec<u8> {
        let size = body.len() as u32;
        let mut tag = vec![kind];
        tag.extend_from_slice(&size.to_be_bytes()[1..]);
        tag.extend_from_slice(&timestamp.to_be_bytes()[1..]);
        tag.push((timestamp >> 24) as u8);
        tag.extend_from_slice(&[0, 0, 0]);
        tag.extend_from_slice(body);
        tag.extend_from_slice(&(TAG_HEADER as u32 + size).to_be_bytes());
        tag
    }

    fn config() -> Vec<u8> {
        let mut body = vec![0x17, 0, 0, 0, 0, 1, 0x64, 0x00, 0x1f, 0xff, 0xe1];
        body.extend_from_slice(&(SPS.len() as u16).to_be_bytes());
        body.extend_from_slice(&SPS);
        body.push(1);
        body.extend_from_slice(&(PPS.len() as u16).to_be_bytes());
        body.extend_from_slice(&PPS);
        body
    }

    fn nalus(key: bool, nals: &[&[u8]]) -> Vec<u8> {
        let mut body = vec![if key { 0x17 } else { 0x27 }, 1, 0, 0, 0];
        for nal in nals {
            body.extend_from_slice(&(nal.len() as u32).to_be_bytes());
            body.extend_from_slice(nal);
        }
        body
    }

    fn stream() -> Vec<u8> {
        let mut bytes = header();
        bytes.extend(tag(18, 0, b"onMetaData"));
        bytes.extend(tag(8, 0, &[0xaf, 0x00, 0x12, 0x10]));
        bytes.extend(tag(TAG_VIDEO, 0, &config()));
        bytes.extend(tag(TAG_VIDEO, 40, &nalus(true, &[&IDR])));
        bytes.extend(tag(TAG_VIDEO, 73, &nalus(false, &[&SLICE, &SLICE])));
        bytes
    }

    fn annexb(nals: &[&[u8]]) -> Vec<u8> {
        nals.iter()
            .flat_map(|nal| START_CODE.iter().chain(nal.iter()).copied())
            .collect()
    }

    #[test]
    fn keyframes_get_the_parameter_sets_in_front() {
        let frames = FlvParser::new().push(&stream()).unwrap();
        assert_eq!(
            frames,
            vec![
                VideoFrame {
                    annexb: annexb(&[&SPS, &PPS, &IDR]),
                    key: true,
                    timestamp_ms: 40,
                },
                VideoFrame {
                    annexb: annexb(&[&SLICE, &SLICE]),
                    key: false,
                    timestamp_ms: 73,
                },
            ]
        );
    }

    #[test]
    fn chunk_sizes_do_not_matter() {
        let whole = FlvParser::new().push(&stream()).unwrap();
        for size in [1, 2, 3, 7, 13, 50] {
            let mut parser = FlvParser::new();
            let mut frames = Vec::new();
            for chunk in stream().chunks(size) {
                frames.extend(parser.push(chunk).unwrap());
            }
            assert_eq!(frames, whole, "每块 {size} 字节");
        }
    }

    #[test]
    fn video_before_the_configuration_is_skipped() {
        let mut bytes = header();
        bytes.extend(tag(TAG_VIDEO, 0, &nalus(true, &[&IDR])));
        bytes.extend(tag(TAG_VIDEO, 33, &config()));
        bytes.extend(tag(TAG_VIDEO, 66, &nalus(true, &[&IDR])));
        let frames = FlvParser::new().push(&bytes).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].timestamp_ms, 66);
    }

    #[test]
    fn the_extended_timestamp_byte_is_the_top_byte() {
        let mut bytes = header();
        bytes.extend(tag(TAG_VIDEO, 0, &config()));
        bytes.extend(tag(TAG_VIDEO, 0x0100_0005, &nalus(true, &[&IDR])));
        let frames = FlvParser::new().push(&bytes).unwrap();
        assert_eq!(frames[0].timestamp_ms, 0x0100_0005);
    }

    #[test]
    fn rejects_other_formats() {
        assert_eq!(
            FlvParser::new().push(b"<html>maintenance</html>"),
            Err(FlvError::NotFlv)
        );
        for first in [0x1c, 0x9c] {
            let mut bytes = header();
            bytes.extend(tag(TAG_VIDEO, 0, &[first, 0, 0, 0, 0]));
            assert_eq!(
                FlvParser::new().push(&bytes),
                Err(FlvError::NotH264),
                "{first:#x}"
            );
        }
    }

    #[test]
    fn absurd_header_lengths_are_rejected() {
        for len in [0u32, 8, 1025, u32::MAX] {
            let mut bytes = b"FLV".to_vec();
            bytes.extend_from_slice(&len.to_be_bytes());
            assert_eq!(
                FlvParser::new().push(&bytes),
                Err(FlvError::NotFlv),
                "{len}"
            );
        }
    }

    #[test]
    fn truncated_configuration_is_an_error() {
        let mut body = config();
        body.truncate(body.len() - 2);
        let mut bytes = header();
        bytes.extend(tag(TAG_VIDEO, 0, &body));
        assert_eq!(FlvParser::new().push(&bytes), Err(FlvError::BadConfig));
    }
}
