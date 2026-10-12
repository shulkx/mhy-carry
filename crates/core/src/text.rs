//! 外部文本（窗口标题、服务器返回的应用名等）显示前的清理。

/// 把控制字符和双向文本控制符换成空格，再去掉首尾空白。
///
/// 窗口标题这类文本来自别的程序。里面的回车、ESC 这类字符打到终端上，可以改写
/// 已经显示的内容，比如伪造确认提示；双向文本控制符可以让文字倒着显示。
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || is_bidi_control(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_control_characters() {
        assert_eq!(printable("a\rb\u{1b}[2Jc\n"), "a b [2Jc");
    }

    #[test]
    fn replaces_bidi_controls() {
        assert_eq!(printable("abc\u{202E}gpj.exe"), "abc gpj.exe");
    }

    #[test]
    fn keeps_ordinary_text() {
        assert_eq!(
            printable("某直播间 - Google Chrome（chrome.exe）"),
            "某直播间 - Google Chrome（chrome.exe）"
        );
    }
}
