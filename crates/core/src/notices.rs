pub const NOTICE_SUMMARY_CHARS: usize = 72;

fn one_line(message: impl Into<String>) -> String {
    message
        .into()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn notice_summary(message: &str, fallback: &str) -> String {
    let message = one_line(message);
    if message.chars().count() <= NOTICE_SUMMARY_CHARS {
        return message;
    }
    let prefix = message
        .char_indices()
        .find(|(_, ch)| matches!(ch, ':' | '\u{2014}'))
        .map(|(index, ch)| message[..index + ch.len_utf8()].trim())
        .filter(|prefix| prefix.chars().count() >= 8);
    prefix.unwrap_or(fallback).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_stay_short() {
        let detail = format!("Error:\n{}", "C:\\very\\long\\path\\".repeat(20));
        assert_eq!(notice_summary(&detail, "Error"), "Error");
        assert_eq!(notice_summary("Short", "Error"), "Short");
    }
}
