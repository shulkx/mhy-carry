//! 终端里的 y/N 确认。默认是“否”，只有明确输入 y 才算同意。

use std::io::{self, BufRead, Write};

use mhy_carry_platform::console;

pub fn confirm(question: &str) -> io::Result<bool> {
    // 只认看到问题之后敲的回答，提问前多按的 y 回车不算。
    console::discard_typed_ahead();
    print!("{question} [y/N] ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(is_yes(&line))
}

fn is_yes(answer: &str) -> bool {
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_y_and_yes_in_any_case() {
        for answer in ["y", "Y", "yes", "YES", " y\r\n"] {
            assert!(is_yes(answer), "{answer:?}");
        }
    }

    #[test]
    fn anything_else_means_no() {
        for answer in ["", "\n", "n", "no", "好", "yy"] {
            assert!(!is_yes(answer), "{answer:?}");
        }
    }
}
