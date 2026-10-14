//! 直播流实验：cargo run --release -- <B站房间号> [秒数]
//! 匿名取 B 站 HTTP-FLV（H.264）流，边拆包边用 Media Foundation 解码，统计各步耗时。
//! 最后一帧的灰度图存成 last-frame.png（不提交）。

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";

struct Frame {
    width: u32,
    height: u32,
    luma: Vec<u8>,
}

// ---------------- FLV 拆包 ----------------

#[derive(Default)]
struct Flv {
    buf: Vec<u8>,
    header_done: bool,
    nal_len: usize,
    sps_pps: Vec<u8>,
}

enum Packet {
    /// Annex B 格式的一帧，是否关键帧，时间戳（毫秒）。
    Video { annexb: Vec<u8>, key: bool, ts: u32 },
}

impl Flv {
    fn push(&mut self, bytes: &[u8]) -> Vec<Packet> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut pos = 0;
        if !self.header_done {
            if self.buf.len() < 13 {
                return out;
            }
            assert_eq!(&self.buf[..3], b"FLV", "不是 FLV");
            let header = u32::from_be_bytes(self.buf[5..9].try_into().unwrap()) as usize;
            pos = header + 4;
            self.header_done = true;
        }
        loop {
            if self.buf.len() < pos + 11 {
                break;
            }
            let kind = self.buf[pos];
            let size = u32::from_be_bytes([0, self.buf[pos + 1], self.buf[pos + 2], self.buf[pos + 3]]) as usize;
            let ts = u32::from_be_bytes([self.buf[pos + 7], self.buf[pos + 4], self.buf[pos + 5], self.buf[pos + 6]]);
            if self.buf.len() < pos + 11 + size + 4 {
                break;
            }
            let data = self.buf[pos + 11..pos + 11 + size].to_vec();
            pos += 11 + size + 4;
            if kind == 9 && data.len() > 5 {
                let frame_type = data[0] >> 4;
                let codec = data[0] & 0x0f;
                if codec != 7 {
                    continue;
                }
                match data[1] {
                    0 => self.read_config(&data[5..]),
                    1 => {
                        let mut annexb = Vec::new();
                        let key = frame_type == 1;
                        if key {
                            annexb.extend_from_slice(&self.sps_pps);
                        }
                        let mut p = 5;
                        while p + self.nal_len <= data.len() {
                            let mut n = 0usize;
                            for b in &data[p..p + self.nal_len] {
                                n = (n << 8) | *b as usize;
                            }
                            p += self.nal_len;
                            if p + n > data.len() {
                                break;
                            }
                            annexb.extend_from_slice(&[0, 0, 0, 1]);
                            annexb.extend_from_slice(&data[p..p + n]);
                            p += n;
                        }
                        out.push(Packet::Video { annexb, key, ts });
                    }
                    _ => {}
                }
            }
        }
        self.buf.drain(..pos);
        out
    }

    fn read_config(&mut self, c: &[u8]) {
        self.nal_len = (c[4] & 0x03) as usize + 1;
        self.sps_pps.clear();
        let mut p = 6;
        let sps_count = c[5] & 0x1f;
        for _ in 0..sps_count {
            let n = u16::from_be_bytes([c[p], c[p + 1]]) as usize;
            self.sps_pps.extend_from_slice(&[0, 0, 0, 1]);
            self.sps_pps.extend_from_slice(&c[p + 2..p + 2 + n]);
            p += 2 + n;
        }
        let pps_count = c[p];
        p += 1;
        for _ in 0..pps_count {
            let n = u16::from_be_bytes([c[p], c[p + 1]]) as usize;
            self.sps_pps.extend_from_slice(&[0, 0, 0, 1]);
            self.sps_pps.extend_from_slice(&c[p + 2..p + 2 + n]);
            p += 2 + n;
        }
        println!("AVC 配置：NAL 长度 {} 字节，SPS/PPS {} 字节", self.nal_len, self.sps_pps.len());
    }
}

// ---------------- Media Foundation 解码 ----------------

struct Decoder {
    mft: IMFTransform,
    width: u32,
    height: u32,
    stride: u32,
}

impl Decoder {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
            MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
            let attrs = mft.GetAttributes()?;
            attrs.SetUINT32(&MF_LOW_LATENCY, 1)?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            mft.SetInputType(0, &input, 0)?;
            let mut decoder = Self { mft, width: 0, height: 0, stride: 0 };
            decoder.choose_output()?;
            decoder.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            decoder.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(decoder)
        }
    }

    fn choose_output(&mut self) -> windows::core::Result<()> {
        unsafe {
            let mut index = 0;
            loop {
                let t = self.mft.GetOutputAvailableType(0, index)?;
                if t.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.mft.SetOutputType(0, &t, 0)?;
                    let size = t.GetUINT64(&MF_MT_FRAME_SIZE)?;
                    self.width = (size >> 32) as u32;
                    self.height = size as u32;
                    self.stride = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).map(|s| s as i32).unwrap_or(self.width as i32).unsigned_abs();
                    println!("解码输出：NV12 {}x{}，行距 {}", self.width, self.height, self.stride);
                    return Ok(());
                }
                index += 1;
            }
        }
    }

    fn decode(&mut self, annexb: &[u8], ts_ms: u32) -> windows::core::Result<Vec<Frame>> {
        let mut frames = Vec::new();
        unsafe {
            let buffer = MFCreateMemoryBuffer(annexb.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(annexb.as_ptr(), ptr, annexb.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(annexb.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(ts_ms as i64 * 10_000)?;
            if let Err(e) = self.mft.ProcessInput(0, &sample, 0) {
                if e.code() != MF_E_NOTACCEPTING {
                    return Err(e);
                }
                self.drain(&mut frames)?;
                self.mft.ProcessInput(0, &sample, 0)?;
            }
            self.drain(&mut frames)?;
        }
        Ok(frames)
    }

    unsafe fn drain(&mut self, frames: &mut Vec<Frame>) -> windows::core::Result<()> {
        unsafe {
            loop {
                let info = self.mft.GetOutputStreamInfo(0)?;
                let out_buffer = MFCreateMemoryBuffer(info.cbSize)?;
                let out_sample = MFCreateSample()?;
                out_sample.AddBuffer(&out_buffer)?;
                let mut out = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(Some(out_sample)),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = self.mft.ProcessOutput(0, &mut out, &mut status);
                ManuallyDrop::drop(&mut out[0].pSample);
                ManuallyDrop::drop(&mut out[0].pEvents);
                match result {
                    Ok(()) => {
                        let mut ptr = std::ptr::null_mut();
                        let mut len = 0u32;
                        out_buffer.Lock(&mut ptr, None, Some(&mut len))?;
                        let data = std::slice::from_raw_parts(ptr, len as usize);
                        let mut luma = Vec::with_capacity((self.width * self.height) as usize);
                        for row in 0..self.height as usize {
                            let start = row * self.stride as usize;
                            luma.extend_from_slice(&data[start..start + self.width as usize]);
                        }
                        out_buffer.Unlock()?;
                        frames.push(Frame { width: self.width, height: self.height, luma });
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.choose_output()?,
                    Err(e) => return Err(e),
                }
            }
        }
    }
}


// ---------------- 自测：编码一张带二维码的图，再解码识别 ----------------

/// 用系统自带的 H.264 编码器把几帧 NV12 编成 Annex B。返回每个输出包。
fn encode(width: u32, height: u32, luma: &[u8], count: u32) -> windows::core::Result<Vec<Vec<u8>>> {
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)?;
        let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)?;
        let output = MFCreateMediaType()?;
        output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        output.SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000)?;
        output.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
        output.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
        output.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        output.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        mft.SetOutputType(0, &output, 0)?;
        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        input.SetUINT64(&MF_MT_FRAME_SIZE, ((width as u64) << 32) | height as u64)?;
        input.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
        input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
        input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        mft.SetInputType(0, &input, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
        mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;

        let mut nv12 = luma.to_vec();
        nv12.resize(luma.len() * 3 / 2, 128);
        let mut packets = Vec::new();
        for i in 0..count {
            let buffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
            let mut ptr = std::ptr::null_mut();
            buffer.Lock(&mut ptr, None, None)?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(nv12.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            sample.SetSampleTime(i as i64 * 333_333)?;
            sample.SetSampleDuration(333_333)?;
            mft.ProcessInput(0, &sample, 0)?;
            drain_encoder(&mft, &mut packets)?;
        }
        mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        drain_encoder(&mft, &mut packets)?;
        Ok(packets)
    }
}

unsafe fn drain_encoder(mft: &IMFTransform, packets: &mut Vec<Vec<u8>>) -> windows::core::Result<()> {
    unsafe {
        loop {
            let info = mft.GetOutputStreamInfo(0)?;
            let provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
            let sample = if provides {
                None
            } else {
                let buffer = MFCreateMemoryBuffer(info.cbSize.max(1 << 20))?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buffer)?;
                Some(sample)
            };
            let mut out = [MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            }];
            let mut status = 0u32;
            let result = mft.ProcessOutput(0, &mut out, &mut status);
            let sample = ManuallyDrop::take(&mut out[0].pSample);
            ManuallyDrop::drop(&mut out[0].pEvents);
            match result {
                Ok(()) => {
                    let buffer = sample.unwrap().ConvertToContiguousBuffer()?;
                    let mut ptr = std::ptr::null_mut();
                    let mut len = 0u32;
                    buffer.Lock(&mut ptr, None, Some(&mut len))?;
                    packets.push(std::slice::from_raw_parts(ptr, len as usize).to_vec());
                    buffer.Unlock()?;
                }
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    }
}

fn selftest() {
    let (width, height) = (640u32, 480u32);
    let text = "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56";
    // 灰色背景上画一个二维码：每个模块 6 像素，四周 4 个模块白边。
    let qr = qrcode_like(text);
    let mut luma = vec![120u8; (width * height) as usize];
    let (scale, quiet, left, top) = (6u32, 4u32, 100u32, 60u32);
    let size = qr.len() as u32;
    for y in 0..(size + 2 * quiet) * scale {
        for x in 0..(size + 2 * quiet) * scale {
            let (mx, my) = (x / scale, y / scale);
            let dark = mx >= quiet && my >= quiet && mx < quiet + size && my < quiet + size
                && qr[(my - quiet) as usize][(mx - quiet) as usize];
            luma[((top + y) * width + left + x) as usize] = if dark { 0 } else { 255 };
        }
    }
    let packets = encode(width, height, &luma, 10).expect("编码");
    println!("编码出 {} 个包，第一个包开头：{:02x?}", packets.len(), &packets[0][..8.min(packets[0].len())]);
    let mut decoder = Decoder::new().expect("创建解码器");
    let mut last = None;
    for (i, packet) in packets.iter().enumerate() {
        for frame in decoder.decode(packet, i as u32 * 33).expect("解码") {
            last = Some(frame);
        }
    }
    let frame = last.expect("一帧都没解出来");
    println!("解出 {}x{}", frame.width, frame.height);
    let found = rxing::helpers::detect_multiple_in_luma(frame.luma, frame.width, frame.height).expect("识别");
    println!("识别结果：{:?}", found.iter().map(|r| r.getText().to_owned()).collect::<Vec<_>>());
}

// ---------------- 取流 ----------------

async fn stream_url(http: &reqwest::Client, room: &str) -> String {
    let get = |url: String| async {
        let bytes = http.get(url).send().await.unwrap().bytes().await.unwrap();
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()
    };
    let init = get(format!("https://api.live.bilibili.com/room/v1/Room/room_init?id={room}")).await;
    let room_id = init["data"]["room_id"].as_u64().unwrap();
    println!("真实房间号 {room_id}，live_status {}", init["data"]["live_status"]);
    let info = get(format!("https://api.live.bilibili.com/xlive/web-room/v2/index/getRoomPlayInfo?room_id={room_id}&protocol=0,1&format=0,1,2&codec=0&qn=10000&platform=web")).await;
    for stream in info["data"]["playurl_info"]["playurl"]["stream"].as_array().unwrap() {
        if stream["protocol_name"] != "http_stream" {
            continue;
        }
        for format in stream["format"].as_array().unwrap() {
            if format["format_name"] != "flv" {
                continue;
            }
            let codec = &format["codec"][0];
            let url_info = &codec["url_info"][0];
            println!("FLV avc，画质 {}", codec["current_qn"]);
            return format!("{}{}{}", url_info["host"].as_str().unwrap(), codec["base_url"].as_str().unwrap(), url_info["extra"].as_str().unwrap());
        }
    }
    panic!("没有 FLV 流");
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let room = args.get(1).expect("用法：live-bench <房间号> [秒数] | --selftest");
    if room == "--selftest" {
        selftest();
        return;
    }
    let seconds: u64 = args.get(2).map_or(20, |s| s.parse().unwrap());
    let http = reqwest::Client::builder()
        .user_agent(UA)
        .default_headers([(reqwest::header::REFERER, "https://live.bilibili.com/".parse().unwrap())].into_iter().collect())
        .build()
        .unwrap();

    let started = Instant::now();
    let url = stream_url(&http, room).await;
    println!("取到地址：{} ms", started.elapsed().as_millis());

    let mut response = http.get(&url).send().await.unwrap();
    println!("流的 HTTP 状态 {}，响应头到达：{} ms", response.status(), started.elapsed().as_millis());
    let mut flv = Flv::default();
    let mut decoder = Decoder::new().expect("创建解码器");
    let (mut bytes, mut packets, mut keyframes) = (0usize, 0usize, 0usize);
    let mut decode_times = Vec::new();
    let mut first_frame_at = None;
    let mut last: Option<Frame> = None;
    let mut frames_total = 0usize;
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        let Some(chunk) = response.chunk().await.unwrap() else { break };
        bytes += chunk.len();
        for Packet::Video { annexb, key, ts } in flv.push(&chunk) {
            packets += 1;
            keyframes += key as usize;
            let t = Instant::now();
            let frames = decoder.decode(&annexb, ts).expect("解码");
            decode_times.push(t.elapsed());
            if !frames.is_empty() && first_frame_at.is_none() {
                first_frame_at = Some(started.elapsed());
            }
            frames_total += frames.len();
            if let Some(frame) = frames.into_iter().last() {
                last = Some(frame);
            }
        }
    }
    let elapsed = deadline.saturating_duration_since(Instant::now());
    let _ = elapsed;
    decode_times.sort();
    let avg = decode_times.iter().sum::<Duration>().as_secs_f64() * 1000.0 / decode_times.len().max(1) as f64;
    println!("收到 {} KB，视频包 {packets}（关键帧 {keyframes}），解码出 {frames_total} 帧", bytes / 1024);
    println!("第一帧画面：{:?}", first_frame_at);
    println!(
        "每包解码耗时：平均 {avg:.2} ms，中位 {:?}，最慢 {:?}",
        decode_times.get(decode_times.len() / 2),
        decode_times.last()
    );

    let frame = last.expect("一帧都没解出来");
    // 识别耗时：同一帧识别 5 次。
    let mut times = Vec::new();
    for _ in 0..5 {
        let t = Instant::now();
        let _ = rxing::helpers::detect_multiple_in_luma(frame.luma.clone(), frame.width, frame.height);
        times.push(t.elapsed());
    }
    println!("rxing 识别一帧 {}x{}：{:?}", frame.width, frame.height, times);
    image::GrayImage::from_raw(frame.width, frame.height, frame.luma)
        .unwrap()
        .save("last-frame.png")
        .unwrap();
    println!("最后一帧存为 last-frame.png");
}

/// 用 qrcode 生成模块矩阵（true 是深色）。
fn qrcode_like(text: &str) -> Vec<Vec<bool>> {
    let code = qrcode::QrCode::new(text.as_bytes()).unwrap();
    let width = code.width();
    let colors = code.to_colors();
    (0..width)
        .map(|y| (0..width).map(|x| colors[y * width + x] == qrcode::Color::Dark).collect())
        .collect()
}
