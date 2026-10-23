use crate::prompts;
use serde_json::Value;

const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

pub fn token(label: &str, start: &str, end: &str, text: &str) -> String {
    let body = serde_json::json!({ "label": label, "start": start, "end": end, "text": text }).to_string();
    format!("{OPEN}{body}{CLOSE}")
}

pub fn expand(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(i) = rest.find(OPEN) {
        out.push_str(&rest[..i]);
        let after = &rest[i + OPEN.len_utf8()..];
        let Some(j) = after.find(CLOSE) else {
            out.push_str(&rest[i..]);
            return out;
        };
        match render(&after[..j]) {
            Some(sentence) => out.push_str(&sentence),
            None => {
                out.push(OPEN);
                out.push_str(&after[..j]);
                out.push(CLOSE);
            }
        }
        rest = &after[j + CLOSE.len_utf8()..];
    }
    out.push_str(rest);
    out
}

fn render(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    let label = v.get("label")?.as_str()?;
    let start = v.get("start")?.as_str()?;
    let end = v.get("end")?.as_str()?;
    let text = v.get("text")?.as_str()?;
    let sentence = prompts::render("agent/quote.j2", minijinja::context! { label, start, end, text });
    Some(sentence.trim_end_matches('\n').to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quote_expands_and_times_stay_as_written() {
        let token = token("@s1", "1:40", "2:29", "Отчёт в пятницу");
        let text = format!("а {token} и [0:00–30:00]?");
        assert_eq!(expand(&text), "а (quoting @s1 1:40–2:29: \"Отчёт в пятницу\") и [0:00–30:00]?");
        assert_eq!(expand("просто [1:40]"), "просто [1:40]");
        assert_eq!(expand("сломанный \u{E000}нет"), "сломанный \u{E000}нет");
    }
}
