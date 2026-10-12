//! 截图目标的纯计算部分：进程名匹配、窗口落在显示器上的哪一块。不调用系统 API。

/// 屏幕上的一个矩形，单位是物理像素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// 窗口落在显示器上的那一块，坐标相对于显示器左上角。窗口完全不在这个显示器上时返回 None。
pub fn visible_region(window: Rect, monitor: Rect) -> Option<Rect> {
    let (wx, wy) = (i64::from(window.x), i64::from(window.y));
    let (mx, my) = (i64::from(monitor.x), i64::from(monitor.y));
    let left = wx.max(mx);
    let top = wy.max(my);
    let right = (wx + i64::from(window.width)).min(mx + i64::from(monitor.width));
    let bottom = (wy + i64::from(window.height)).min(my + i64::from(monitor.height));
    if right <= left || bottom <= top {
        return None;
    }
    Some(Rect {
        x: (left - mx) as i32,
        y: (top - my) as i32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

/// 路径里的文件名，比如 `E:\games\YuanShen.exe` → `YuanShen.exe`。
pub fn exe_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// 进程的 exe 文件名是否在列表里。Windows 的文件名不区分大小写。
pub fn is_listed_exe(exe: &str, names: &[&str]) -> bool {
    names.iter().any(|name| name.eq_ignore_ascii_case(exe))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: Rect = Rect {
        x: 0,
        y: 0,
        width: 3840,
        height: 2160,
    };

    #[test]
    fn window_fully_on_the_monitor() {
        let window = Rect {
            x: 1459,
            y: 145,
            width: 1922,
            height: 1126,
        };
        assert_eq!(visible_region(window, MONITOR), Some(window));
    }

    #[test]
    fn window_hanging_off_the_left_and_bottom_is_clipped() {
        let window = Rect {
            x: -100,
            y: 2000,
            width: 500,
            height: 400,
        };
        assert_eq!(
            visible_region(window, MONITOR),
            Some(Rect {
                x: 0,
                y: 2000,
                width: 400,
                height: 160,
            })
        );
    }

    #[test]
    fn coordinates_are_relative_to_a_secondary_monitor() {
        let left_monitor = Rect {
            x: -1920,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let window = Rect {
            x: -1000,
            y: 100,
            width: 800,
            height: 600,
        };
        assert_eq!(
            visible_region(window, left_monitor),
            Some(Rect {
                x: 920,
                y: 100,
                width: 800,
                height: 600,
            })
        );
    }

    #[test]
    fn window_on_another_monitor_has_no_region() {
        let window = Rect {
            x: 4000,
            y: 0,
            width: 800,
            height: 600,
        };
        assert_eq!(visible_region(window, MONITOR), None);
    }

    #[test]
    fn minimized_window_position_has_no_region() {
        // 最小化的窗口会被 Windows 挪到 (-32000, -32000)。
        let window = Rect {
            x: -32000,
            y: -32000,
            width: 160,
            height: 28,
        };
        assert_eq!(visible_region(window, MONITOR), None);
    }

    #[test]
    fn exe_name_takes_the_file_name() {
        assert_eq!(
            exe_name(r"E:\miHoYo Launcher\games\Genshin Impact Game\YuanShen.exe"),
            "YuanShen.exe"
        );
        assert_eq!(exe_name("YuanShen.exe"), "YuanShen.exe");
    }

    #[test]
    fn exe_names_compare_case_insensitively() {
        assert!(is_listed_exe("yuanshen.EXE", &["YuanShen.exe"]));
        assert!(!is_listed_exe("YuanShen.exe.bak", &["YuanShen.exe"]));
        assert!(!is_listed_exe("GenshinImpact.exe", &["YuanShen.exe"]));
    }
}
