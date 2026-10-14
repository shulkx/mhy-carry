//! H.264 解码（设计文档第 6.5 节）：用 Windows Media Foundation 自带的解码器，只取亮度平面。
//! COM 对象只能在创建它的线程里用，所以解码器要在拉流线程里创建。

use std::marker::PhantomData;
use std::mem::ManuallyDrop;

use mhy_carry_core::capture::Frame;
use mhy_carry_core::video::H264Decoder;
use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::Media::MediaFoundation::{
    CLSID_MSH264DecoderMFT, IMFMediaType, IMFTransform, MF_E_NOTACCEPTING,
    MF_E_TRANSFORM_NEED_MORE_INPUT, MF_E_TRANSFORM_STREAM_CHANGE, MF_LOW_LATENCY,
    MF_MT_DEFAULT_STRIDE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE, MF_MT_MINIMUM_DISPLAY_APERTURE,
    MF_MT_SUBTYPE, MF_VERSION, MFCreateMediaType, MFCreateMemoryBuffer, MFCreateSample,
    MFMediaType_Video, MFSTARTUP_NOSOCKET, MFShutdown, MFStartup,
    MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER,
    MFVideoArea, MFVideoFormat_H264, MFVideoFormat_NV12,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};

/// 本线程的 COM 初始化，丢弃时撤销。线程已经按别的模式初始化过 COM 时，直接用，什么也不撤销。
struct Com {
    initialized: bool,
    /// 要在初始化的那个线程里撤销，所以不能挪到别的线程。
    _thread: PhantomData<*const ()>,
}

impl Com {
    fn init() -> windows::core::Result<Self> {
        // SAFETY: 成功时由 Drop 里的 CoUninitialize 配对。
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let initialized = result != RPC_E_CHANGED_MODE;
        if initialized {
            result.ok()?;
        }
        Ok(Self {
            initialized,
            _thread: PhantomData,
        })
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.initialized {
            // SAFETY: 和 init 里成功的 CoInitializeEx 配对，在同一个线程里。
            unsafe { CoUninitialize() };
        }
    }
}

/// Media Foundation 的启动，丢弃时关闭。
struct MediaFoundation;

impl MediaFoundation {
    fn start() -> windows::core::Result<Self> {
        // SAFETY: 成功时由 Drop 里的 MFShutdown 配对。
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET)? };
        Ok(Self)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        // SAFETY: 和 start 里成功的 MFStartup 配对。
        unsafe {
            let _ = MFShutdown();
        }
    }
}

/// 系统自带的 H.264 解码器。
pub struct MfH264Decoder {
    /// 字段按声明的顺序丢弃：先释放解码器，再关 Media Foundation，最后撤销 COM 初始化。
    mft: IMFTransform,
    /// 解码输出的尺寸（可能对齐到 16 的倍数，比如 1088）。
    width: u32,
    height: u32,
    /// 真正显示的区域，裁掉对齐多出来的行。
    visible_width: u32,
    visible_height: u32,
    /// 每行的字节数。
    stride: usize,
    _media: MediaFoundation,
    _com: Com,
}

impl MfH264Decoder {
    /// 创建解码器。电脑上没有 H.264 解码器（比如 Windows N 版没装媒体功能包）时返回原因。
    pub fn new() -> Result<Self, String> {
        Self::create().map_err(|error| {
            format!(
                "创建 H.264 解码器失败，这台电脑可能缺少媒体功能（Windows N 版要装“媒体功能包”）：{error}"
            )
        })
    }

    fn create() -> windows::core::Result<Self> {
        // 中途出错时，已经做了的初始化随着这两个变量丢弃而撤销。
        let com = Com::init()?;
        let media = MediaFoundation::start()?;
        // SAFETY: 都是 COM 和 Media Foundation 的普通调用，参数都是本函数里创建的对象。
        unsafe {
            let mft: IMFTransform =
                CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER)?;
            // 低延迟：解出一帧就交出来，不为重排序攒帧。
            mft.GetAttributes()?.SetUINT32(&MF_LOW_LATENCY, 1)?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
            mft.SetInputType(0, &input, 0)?;
            let mut decoder = Self {
                mft,
                width: 0,
                height: 0,
                visible_width: 0,
                visible_height: 0,
                stride: 0,
                _media: media,
                _com: com,
            };
            decoder.choose_output()?;
            let mft = &decoder.mft;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            Ok(decoder)
        }
    }

    /// 选 NV12 输出，记下尺寸。开始时和分辨率变化时调用。
    fn choose_output(&mut self) -> windows::core::Result<()> {
        // SAFETY: 同上。
        unsafe {
            let mut index = 0;
            loop {
                let candidate = self.mft.GetOutputAvailableType(0, index)?;
                if candidate.GetGUID(&MF_MT_SUBTYPE)? == MFVideoFormat_NV12 {
                    self.mft.SetOutputType(0, &candidate, 0)?;
                    let size = candidate.GetUINT64(&MF_MT_FRAME_SIZE)?;
                    self.width = (size >> 32) as u32;
                    self.height = size as u32;
                    self.stride = candidate
                        .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                        .map_or(self.width, |stride| (stride as i32).unsigned_abs())
                        as usize;
                    (self.visible_width, self.visible_height) =
                        visible_size(&candidate, self.width, self.height);
                    return Ok(());
                }
                index += 1;
            }
        }
    }

    /// 取出解码器里所有解好的帧，只留最后一帧。
    fn drain(&mut self) -> windows::core::Result<Option<Frame>> {
        let mut latest = None;
        // SAFETY: 输出缓冲区由我们分配，锁住后只在长度范围内读。
        unsafe {
            loop {
                let info = self.mft.GetOutputStreamInfo(0)?;
                let buffer = MFCreateMemoryBuffer(info.cbSize)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buffer)?;
                let mut output = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(Some(sample)),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = self.mft.ProcessOutput(0, &mut output, &mut status);
                ManuallyDrop::drop(&mut output[0].pSample);
                ManuallyDrop::drop(&mut output[0].pEvents);
                match result {
                    Ok(()) => {
                        let mut data = std::ptr::null_mut();
                        let mut len = 0u32;
                        buffer.Lock(&mut data, None, Some(&mut len))?;
                        let bytes = std::slice::from_raw_parts(data, len as usize);
                        latest = self.luma(bytes);
                        buffer.Unlock()?;
                    }
                    Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                        return Ok(latest);
                    }
                    Err(error) if error.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        self.choose_output()?;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    /// 从 NV12 里取出显示区域的亮度平面。缓冲区比预期短时放弃这一帧。
    fn luma(&self, nv12: &[u8]) -> Option<Frame> {
        let (width, height) = (self.visible_width as usize, self.visible_height as usize);
        if width == 0 || height == 0 || nv12.len() < self.stride * (height - 1) + width {
            return None;
        }
        let mut luma = Vec::with_capacity(width * height);
        for row in 0..height {
            let start = row * self.stride;
            luma.extend_from_slice(&nv12[start..start + width]);
        }
        Some(Frame {
            width: self.visible_width,
            height: self.visible_height,
            luma,
        })
    }

    fn send(&mut self, annexb: &[u8], timestamp_ms: u32) -> windows::core::Result<Option<Frame>> {
        // SAFETY: 输入缓冲区由我们分配，复制的长度就是分配的长度。
        unsafe {
            let buffer = MFCreateMemoryBuffer(annexb.len() as u32)?;
            let mut data = std::ptr::null_mut();
            buffer.Lock(&mut data, None, None)?;
            std::ptr::copy_nonoverlapping(annexb.as_ptr(), data, annexb.len());
            buffer.Unlock()?;
            buffer.SetCurrentLength(annexb.len() as u32)?;
            let sample = MFCreateSample()?;
            sample.AddBuffer(&buffer)?;
            // Media Foundation 的时间单位是 100 纳秒。
            sample.SetSampleTime(i64::from(timestamp_ms) * 10_000)?;
            let mut latest = None;
            if let Err(error) = self.mft.ProcessInput(0, &sample, 0) {
                if error.code() != MF_E_NOTACCEPTING {
                    return Err(error);
                }
                latest = self.drain()?;
                self.mft.ProcessInput(0, &sample, 0)?;
            }
            Ok(self.drain()?.or(latest))
        }
    }
}

/// 显示区域：读 MF_MT_MINIMUM_DISPLAY_APERTURE，没有就用整个输出尺寸。
fn visible_size(media_type: &IMFMediaType, width: u32, height: u32) -> (u32, u32) {
    let mut area = MFVideoArea::default();
    let size = std::mem::size_of::<MFVideoArea>();
    // SAFETY: 缓冲区就是 area 本身，长度是它的大小。
    let read = unsafe {
        media_type.GetBlob(
            &MF_MT_MINIMUM_DISPLAY_APERTURE,
            std::slice::from_raw_parts_mut((&mut area as *mut MFVideoArea).cast::<u8>(), size),
            None,
        )
    };
    let (cx, cy) = (area.Area.cx, area.Area.cy);
    if read.is_ok() && cx > 0 && cy > 0 {
        ((cx as u32).min(width), (cy as u32).min(height))
    } else {
        (width, height)
    }
}

impl H264Decoder for MfH264Decoder {
    fn decode(&mut self, annexb: &[u8], timestamp_ms: u32) -> Result<Option<Frame>, String> {
        self.send(annexb, timestamp_ms)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mhy_carry_core::decode::{QrDecoder, RxingDecoder};
    use windows::Win32::Media::MediaFoundation::{
        CLSID_MSH264EncoderMFT, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_INTERLACE_MODE,
        MF_MT_PIXEL_ASPECT_RATIO, MFT_MESSAGE_COMMAND_DRAIN, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
        MFVideoInterlace_Progressive,
    };

    const URL: &str = "https://user.mihoyo.com/qr_code_in_game.html?app_id=4&biz_key=hk4e_cn&expire=4102444800&ticket=66cf3afb2a1c7a308832bd56";
    const WIDTH: u32 = 640;
    const HEIGHT: u32 = 480;

    /// 灰色背景上画一个二维码：每个模块 6 像素，四周 4 个模块宽的白边。
    fn picture() -> Vec<u8> {
        let code = qrcode::QrCode::new(URL.as_bytes()).unwrap();
        let colors = code.to_colors();
        let modules = code.width() as u32;
        let (scale, quiet, left, top) = (6, 4, 100, 60);
        let mut luma = vec![120u8; (WIDTH * HEIGHT) as usize];
        for y in 0..(modules + 2 * quiet) * scale {
            for x in 0..(modules + 2 * quiet) * scale {
                let (mx, my) = (x / scale, y / scale);
                let inside =
                    mx >= quiet && my >= quiet && mx < quiet + modules && my < quiet + modules;
                let dark = inside
                    && colors[((my - quiet) * modules + mx - quiet) as usize]
                        == qrcode::Color::Dark;
                luma[((top + y) * WIDTH + left + x) as usize] = if dark { 0 } else { 255 };
            }
        }
        luma
    }

    fn video_type(subtype: windows::core::GUID) -> windows::core::Result<IMFMediaType> {
        // SAFETY: 普通的 Media Foundation 调用。
        unsafe {
            let media_type = MFCreateMediaType()?;
            media_type.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            media_type.SetGUID(&MF_MT_SUBTYPE, &subtype)?;
            media_type.SetUINT64(
                &MF_MT_FRAME_SIZE,
                (u64::from(WIDTH) << 32) | u64::from(HEIGHT),
            )?;
            media_type.SetUINT64(&MF_MT_FRAME_RATE, (30u64 << 32) | 1)?;
            media_type.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, (1u64 << 32) | 1)?;
            media_type.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            Ok(media_type)
        }
    }

    /// 用系统 H.264 编码器把同一张图编成几帧 Annex B。
    fn encode(luma: &[u8], count: u32) -> windows::core::Result<Vec<Vec<u8>>> {
        let _com = Com::init()?;
        let _media = MediaFoundation::start()?;
        // SAFETY: 普通的 Media Foundation 调用；缓冲区都由我们分配，读写都在长度范围内。
        unsafe {
            let mft: IMFTransform =
                CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER)?;
            let output = video_type(MFVideoFormat_H264)?;
            output.SetUINT32(&MF_MT_AVG_BITRATE, 4_000_000)?;
            mft.SetOutputType(0, &output, 0)?;
            mft.SetInputType(0, &video_type(MFVideoFormat_NV12)?, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)?;
            let mut nv12 = luma.to_vec();
            nv12.resize(luma.len() * 3 / 2, 128);
            let mut packets = Vec::new();
            for i in 0..count {
                let buffer = MFCreateMemoryBuffer(nv12.len() as u32)?;
                let mut data = std::ptr::null_mut();
                buffer.Lock(&mut data, None, None)?;
                std::ptr::copy_nonoverlapping(nv12.as_ptr(), data, nv12.len());
                buffer.Unlock()?;
                buffer.SetCurrentLength(nv12.len() as u32)?;
                let sample = MFCreateSample()?;
                sample.AddBuffer(&buffer)?;
                sample.SetSampleTime(i64::from(i) * 333_333)?;
                sample.SetSampleDuration(333_333)?;
                mft.ProcessInput(0, &sample, 0)?;
                drain_encoder(&mft, &mut packets)?;
            }
            mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
            drain_encoder(&mft, &mut packets)?;
            Ok(packets)
        }
    }

    unsafe fn drain_encoder(
        mft: &IMFTransform,
        packets: &mut Vec<Vec<u8>>,
    ) -> windows::core::Result<()> {
        // SAFETY: 同 encode。
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
                let mut output = [MFT_OUTPUT_DATA_BUFFER {
                    dwStreamID: 0,
                    pSample: ManuallyDrop::new(sample),
                    dwStatus: 0,
                    pEvents: ManuallyDrop::new(None),
                }];
                let mut status = 0u32;
                let result = mft.ProcessOutput(0, &mut output, &mut status);
                let sample = ManuallyDrop::take(&mut output[0].pSample);
                ManuallyDrop::drop(&mut output[0].pEvents);
                match result {
                    Ok(()) => {
                        let buffer = sample
                            .expect("编码器交出了输出")
                            .ConvertToContiguousBuffer()?;
                        let mut data = std::ptr::null_mut();
                        let mut len = 0u32;
                        buffer.Lock(&mut data, None, Some(&mut len))?;
                        packets.push(std::slice::from_raw_parts(data, len as usize).to_vec());
                        buffer.Unlock()?;
                    }
                    Err(error) if error.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(()),
                    Err(error) => return Err(error),
                }
            }
        }
    }

    #[test]
    fn decodes_what_the_system_encoder_produced() {
        // CI 的 Windows Server 可能没有媒体功能，这时没法测，跳过。
        let Ok(packets) = encode(&picture(), 10) else {
            eprintln!("这台电脑没有 H.264 编码器，跳过");
            return;
        };
        let mut decoder = match MfH264Decoder::new() {
            Ok(decoder) => decoder,
            Err(message) => {
                eprintln!("{message}，跳过");
                return;
            }
        };
        let mut latest = None;
        for (i, packet) in packets.iter().enumerate() {
            if let Some(frame) = decoder.decode(packet, i as u32 * 33).unwrap() {
                latest = Some(frame);
            }
        }
        let frame = latest.expect("一帧都没解出来");
        assert_eq!((frame.width, frame.height), (WIDTH, HEIGHT));
        assert_eq!(RxingDecoder.decode(frame), vec![URL.to_owned()]);
    }

    #[test]
    fn garbage_input_is_an_error_or_nothing_but_never_a_panic() {
        let Ok(mut decoder) = MfH264Decoder::new() else {
            return;
        };
        let result = decoder.decode(&[0, 0, 0, 1, 0x65, 1, 2, 3, 4, 5], 0);
        assert!(matches!(result, Ok(None) | Err(_)), "{result:?}");
    }
}
