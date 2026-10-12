//! Windows 上的截图（设计文档第 6.2、14 节）：xcap 的 GDI 后端，截窗口所在的那块屏幕区域。
//!
//! 截图目标只记窗口句柄或显示器编号，每一帧都重新找一次：窗口关了、最小化了、
//! 显示器断开了，都能马上发现。

use mhy_carry_core::capture::{CaptureError, Frame, FrameSource, SourceInfo, SourceKind};
use mhy_carry_core::games::GameProfile;
use mhy_carry_core::text::printable;
use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::PWSTR;
use xcap::image::{DynamicImage, RgbaImage};
use xcap::{Monitor, Window, XCapError};

use crate::target::{Rect, exe_name, is_listed_exe, visible_region};

/// 进程 exe 的完整路径。
///
/// 不用 xcap 的 `app_name()`：它优先返回 exe 版本信息里的描述，不一定是文件名。
/// 用 `PROCESS_QUERY_LIMITED_INFORMATION` 打开进程，以普通权限运行也能查到以管理员权限运行的游戏。
pub fn process_path(pid: u32) -> Option<String> {
    // SAFETY: 只把栈上缓冲区的指针和长度交给系统调用，句柄用完就关。
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result.ok()?;
        Some(String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

/// 一个可以选来扫描的窗口。
#[derive(Debug, Clone)]
pub struct WindowTarget {
    id: u32,
    /// “标题（进程名）”。
    pub label: String,
}

/// 一个可以选来扫描的显示器。
#[derive(Debug, Clone)]
pub struct MonitorTarget {
    id: u32,
    /// “显示器 名称（宽×高）”。
    pub label: String,
    pub is_primary: bool,
}

/// 列出有标题的可见窗口，给你选“指定窗口”。最小化的窗口也在里面。
pub fn list_windows() -> Result<Vec<WindowTarget>, CaptureError> {
    let mut targets = Vec::new();
    for window in Window::all().map_err(failed)? {
        let title = window.title().unwrap_or_default();
        if title.trim().is_empty() {
            continue;
        }
        let Ok(id) = window.id() else {
            continue;
        };
        let exe = window
            .pid()
            .ok()
            .and_then(process_path)
            .map(|path| exe_name(&path).to_owned())
            .unwrap_or_default();
        targets.push(WindowTarget {
            id,
            label: printable(&format!("{title}（{exe}）")),
        });
    }
    Ok(targets)
}

/// 列出所有显示器。
pub fn list_monitors() -> Result<Vec<MonitorTarget>, CaptureError> {
    let mut targets = Vec::new();
    for monitor in Monitor::all().map_err(failed)? {
        let Ok(id) = monitor.id() else {
            continue;
        };
        let name = monitor
            .friendly_name()
            .or_else(|_| monitor.name())
            .unwrap_or_default();
        let width = monitor.width().unwrap_or(0);
        let height = monitor.height().unwrap_or(0);
        targets.push(MonitorTarget {
            id,
            label: printable(&format!("显示器 {name}（{width}×{height}）")),
            is_primary: monitor.is_primary().unwrap_or(false),
        });
    }
    Ok(targets)
}

/// 自动模式：按游戏配置里的进程名找游戏窗口。
pub struct GameWindowSource {
    games: &'static [GameProfile],
    /// 找到的游戏窗口：窗口句柄和来源描述。
    found: Option<(u32, SourceInfo)>,
}

impl GameWindowSource {
    pub fn new(games: &'static [GameProfile]) -> Self {
        Self { games, found: None }
    }

    fn search(&self) -> Result<Option<(Window, SourceInfo)>, CaptureError> {
        for window in Window::all().map_err(failed)? {
            let Ok(pid) = window.pid() else {
                continue;
            };
            let Some(path) = process_path(pid) else {
                continue;
            };
            let exe = exe_name(&path);
            if let Some(game) = self
                .games
                .iter()
                .find(|game| is_listed_exe(exe, game.process_names))
            {
                let label = format!("{exe}（{}）", game.display_name);
                return Ok(Some((
                    window,
                    SourceInfo::new(SourceKind::GameWindow, &label),
                )));
            }
        }
        Ok(None)
    }
}

impl FrameSource for GameWindowSource {
    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        if let Some((id, _)) = &self.found
            && let Some(window) = find_window(*id)?
        {
            return capture_window(&window);
        }
        // 还没找到，或者窗口关了：重新找。
        self.found = None;
        let (window, info) = self.search()?.ok_or(CaptureError::NotFound)?;
        self.found = Some((window.id().map_err(failed)?, info));
        capture_window(&window)
    }

    fn describe(&self) -> SourceInfo {
        match &self.found {
            Some((_, info)) => info.clone(),
            None => {
                let names: Vec<&str> = self.games.iter().map(|game| game.display_name).collect();
                SourceInfo::new(SourceKind::GameWindow, &names.join("、"))
            }
        }
    }
}

/// 你从列表里选的窗口。
pub struct PickedWindowSource {
    id: u32,
    info: SourceInfo,
}

impl PickedWindowSource {
    pub fn new(target: &WindowTarget) -> Self {
        Self {
            id: target.id,
            info: SourceInfo::new(SourceKind::PickedWindow, &target.label),
        }
    }
}

impl FrameSource for PickedWindowSource {
    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        match find_window(self.id)? {
            Some(window) => capture_window(&window),
            // xcap 的窗口列表不含隐藏的窗口。窗口还在、只是隐藏了（缩到托盘、在别的
            // 虚拟桌面上），就当作暂停；真的关了才停止扫描。
            None if window_exists(self.id) => Err(CaptureError::Minimized),
            None => Err(CaptureError::Gone),
        }
    }

    fn describe(&self) -> SourceInfo {
        self.info.clone()
    }
}

/// 整个显示器。
pub struct MonitorSource {
    id: u32,
    info: SourceInfo,
}

impl MonitorSource {
    pub fn new(target: &MonitorTarget) -> Self {
        Self {
            id: target.id,
            info: SourceInfo::new(SourceKind::Monitor, &target.label),
        }
    }
}

impl FrameSource for MonitorSource {
    fn next_frame(&mut self) -> Result<Frame, CaptureError> {
        let monitor = Monitor::all()
            .map_err(failed)?
            .into_iter()
            .find(|monitor| monitor.id().ok() == Some(self.id))
            .ok_or(CaptureError::Gone)?;
        monitor.capture_image().map(to_frame).map_err(failed)
    }

    fn describe(&self) -> SourceInfo {
        self.info.clone()
    }
}

/// 按窗口句柄找回窗口。窗口关了（或者不再可见）就返回 None。
fn find_window(id: u32) -> Result<Option<Window>, CaptureError> {
    Ok(Window::all()
        .map_err(failed)?
        .into_iter()
        .find(|window| window.id().ok() == Some(id)))
}

/// 窗口句柄是否还有效，隐藏的窗口也算。
fn window_exists(id: u32) -> bool {
    // xcap 的窗口编号是 HWND 截成的 32 位数。窗口句柄只有低 32 位有效，按有符号数扩展回来。
    let hwnd = HWND(id as i32 as isize as *mut c_void);
    // SAFETY: IsWindow 只是查询，传入任何值都安全。
    unsafe { IsWindow(Some(hwnd)).as_bool() }
}

/// 截窗口所在的那块屏幕区域。窗口最小化时返回 `Minimized`。
fn capture_window(window: &Window) -> Result<Frame, CaptureError> {
    if window.is_minimized().map_err(failed)? {
        return Err(CaptureError::Minimized);
    }
    let monitor = window.current_monitor().map_err(failed)?;
    let window_rect = Rect {
        x: window.x().map_err(failed)?,
        y: window.y().map_err(failed)?,
        width: window.width().map_err(failed)?,
        height: window.height().map_err(failed)?,
    };
    let monitor_rect = Rect {
        x: monitor.x().map_err(failed)?,
        y: monitor.y().map_err(failed)?,
        width: monitor.width().map_err(failed)?,
        height: monitor.height().map_err(failed)?,
    };
    let region = visible_region(window_rect, monitor_rect)
        .ok_or_else(|| CaptureError::Failed("窗口不在屏幕上".to_owned()))?;
    monitor
        .capture_region(
            region.x as u32,
            region.y as u32,
            region.width,
            region.height,
        )
        .map(to_frame)
        .map_err(failed)
}

fn to_frame(image: RgbaImage) -> Frame {
    let (width, height) = image.dimensions();
    let luma = DynamicImage::ImageRgba8(image).into_luma8().into_raw();
    Frame {
        width,
        height,
        luma,
    }
}

fn failed(error: XCapError) -> CaptureError {
    CaptureError::Failed(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcap::image::Rgba;

    #[test]
    fn converts_screenshots_to_gray_frames() {
        let image = RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                Rgba([0, 0, 0, 255])
            } else {
                Rgba([255, 255, 255, 255])
            }
        });
        assert_eq!(
            to_frame(image),
            Frame {
                width: 2,
                height: 1,
                luma: vec![0, 255],
            }
        );
    }

    #[test]
    fn window_ids_map_back_to_window_handles() {
        use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
        // SAFETY: 只是取桌面窗口的句柄。
        let desktop = unsafe { GetDesktopWindow() };
        assert!(window_exists(desktop.0 as usize as u32));
        assert!(!window_exists(0));
    }
}
