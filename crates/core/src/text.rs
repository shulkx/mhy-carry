//! 外部文本（窗口标题、服务器返回的应用名等）显示前的清理。

/// 把控制字符和看不见的格式字符换成空格，再去掉首尾空白。
///
/// 窗口标题这类文本来自别的程序。里面的回车、ESC 这类字符打到终端上，可以改写
/// 已经显示的内容，比如伪造确认提示；双向文本控制符可以让文字倒着显示；零宽字符
/// 可以让两段看起来一样的文字其实不同。
pub fn printable(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || is_invisible_format(c) {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

/// 双向文本控制符、零宽字符、行分隔符、软连字符、BOM 等看不见的格式字符。
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{FEFF}'
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
    fn replaces_invisible_format_characters() {
        assert_eq!(
            printable("a\u{200B}b\u{061C}c\u{2028}d\u{FEFF}e\u{00AD}f"),
            "a b c d e f"
        );
    }

    #[test]
    fn keeps_ordinary_text() {
        assert_eq!(
            printable("某直播间 - Google Chrome（chrome.exe）"),
            "某直播间 - Google Chrome（chrome.exe）"
        );
    }
}
