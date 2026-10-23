#[derive(Debug, Clone, PartialEq)]
pub struct Who {
    pub index: u32,
    pub recording: Option<u32>,
    pub name: String,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Parsed {
    pub index: u32,
    pub recording: Option<u32>,
}

pub fn label(index: u32) -> String {
    format!("@s{}", index + 1)
}

pub fn folder_label(index: u32, recording: u32) -> String {
    format!("@s{}#{recording}", index + 1)
}

pub fn parse_label(text: &str) -> Option<Parsed> {
    let s = text.trim().trim_start_matches('@');
    let (num, rec) = match s.split_once('#') {
        Some((n, r)) => (n, Some(r)),
        None => (s, None),
    };
    let n = num.strip_prefix('s')?.parse::<u32>().ok()?;
    if n == 0 {
        return None;
    }
    let recording = match rec {
        Some(r) => {
            let r = r.parse::<u32>().ok()?;
            if r == 0 {
                return None;
            }
            Some(r)
        }
        None => None,
    };
    Some(Parsed { index: n - 1, recording })
}

pub fn shown(name: &str, unnamed: &str, title: &str) -> String {
    let name = if name.trim().is_empty() { unnamed } else { name.trim() };
    if title.is_empty() { format!("@{name}") } else { format!("@{name} ({title})") }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Piece<'a> {
    Text(&'a str),
    Mention { text: String, live: bool },
}

enum Tok<'a> {
    Text(&'a str),
    Speaker { index: u32, recording: Option<u32>, typed: Option<&'a str> },
}

fn boundary_before(text: &str, at: usize) -> bool {
    at == 0 || text[..at].chars().next_back().is_none_or(|c| !c.is_alphanumeric() && c != '_' && c != '@')
}

fn boundary_after(text: &str, at: usize) -> bool {
    text[at..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_')
}

fn model_label_at(text: &str, at: usize) -> Option<(usize, Parsed)> {
    let rest = text.get(at + 1..)?;
    let b = rest.as_bytes();
    if b.first() != Some(&b's') {
        return None;
    }
    let mut j = 1;
    if j >= b.len() || !b[j].is_ascii_digit() {
        return None;
    }
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    let n: u32 = rest[1..j].parse().ok()?;
    if n == 0 {
        return None;
    }
    let mut recording = None;
    if j < b.len() && b[j] == b'#' {
        let start = j + 1;
        let mut k = start;
        if k < b.len() && b[k].is_ascii_digit() {
            while k < b.len() && b[k].is_ascii_digit() {
                k += 1;
            }
            let r: u32 = rest[start..k].parse().ok()?;
            if r == 0 {
                return None;
            }
            recording = Some(r);
            j = k;
        }
    }
    let end = at + 1 + j;
    boundary_after(text, end).then_some((end, Parsed { index: n - 1, recording }))
}

fn eq_ignore_case(a: char, b: char) -> bool {
    a.to_lowercase().zip(b.to_lowercase()).all(|(x, y)| x == y) && a.to_lowercase().count() == b.to_lowercase().count()
}

fn starts_with_ignore_case(text: &str, key: &str) -> bool {
    let mut chars = text.chars();
    key.chars().all(|k| chars.next().is_some_and(|c| eq_ignore_case(c, k)))
}

fn prefix_len(text: &str, chars: usize) -> Option<usize> {
    let mut n = 0;
    for (i, _) in text.char_indices() {
        if n == chars {
            return Some(i);
        }
        n += 1;
    }
    (n == chars).then_some(text.len())
}

fn unique_names(speakers: &[Who]) -> Vec<(String, u32, Option<u32>)> {
    let mut keys: Vec<(String, u32, Option<u32>)> = Vec::new();
    for w in speakers {
        if !w.name.trim().is_empty() {
            keys.push((w.name.trim().to_string(), w.index, w.recording));
        }
        let n = w.index + 1;
        keys.push((format!("Говорящий {n}"), w.index, w.recording));
        keys.push((format!("Speaker {n}"), w.index, w.recording));
    }
    keys.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    keys.dedup_by(|a, b| a.0.to_lowercase() == b.0.to_lowercase() && a.1 == b.1 && a.2 == b.2);
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for (k, _, _) in &keys {
        *counts.entry(k.to_lowercase()).or_default() += 1;
    }
    keys.retain(|k| counts.get(&k.0.to_lowercase()).copied() == Some(1));
    keys.sort_by_key(|k| std::cmp::Reverse(k.0.chars().count()));
    keys
}

fn name_at(text: &str, at: usize, speakers: &[Who]) -> Option<(usize, u32, Option<u32>)> {
    let rest = text.get(at..)?;
    for (key, index, recording) in unique_names(speakers) {
        if starts_with_ignore_case(rest, &key) {
            let end = at + prefix_len(rest, key.chars().count())?;
            if boundary_after(text, end) {
                return Some((end, index, recording));
            }
        }
    }
    None
}

fn tokens<'a>(text: &'a str, speakers: &[Who]) -> Vec<Tok<'a>> {
    let mut out = Vec::new();
    let mut plain = 0;
    let mut i = 0;
    while i < text.len() {
        let hit = text[i..].starts_with('@') && boundary_before(text, i);
        let found = hit.then(|| {
            model_label_at(text, i).map(|(end, p)| (end, p.index, p.recording, None)).or_else(|| name_at(text, i + '@'.len_utf8(), speakers).map(|(end, index, recording)| (end, index, recording, Some(()))))
        });
        if let Some((end, index, recording, typed)) = found.flatten() {
            if plain < i {
                out.push(Tok::Text(&text[plain..i]));
            }
            out.push(Tok::Speaker { index, recording, typed: typed.map(|_| &text[i..end]) });
            i = end;
            plain = i;
            continue;
        }
        i += text[i..].chars().next().map_or(1, char::len_utf8);
    }
    if plain < text.len() {
        out.push(Tok::Text(&text[plain..]));
    }
    out
}

fn resolve(speakers: &[Who], index: u32, recording: Option<u32>) -> Option<&Who> {
    if let Some(r) = recording {
        return speakers.iter().find(|w| w.index == index && w.recording == Some(r));
    }
    let mut hits = speakers.iter().filter(|w| w.index == index);
    let first = hits.next()?;
    match hits.next() {
        None => Some(first),
        Some(_) => speakers.iter().find(|w| w.index == index && w.recording.is_none()),
    }
}

pub fn pieces<'a>(text: &'a str, speakers: &[Who], unnamed: impl Fn(u32) -> String) -> Vec<Piece<'a>> {
    tokens(text, speakers)
        .into_iter()
        .map(|t| match t {
            Tok::Text(s) => Piece::Text(s),
            Tok::Speaker { typed: Some(typed), .. } => Piece::Mention { text: typed.to_string(), live: false },
            Tok::Speaker { index, recording, typed: None } => {
                let who = resolve(speakers, index, recording);
                let name = who.map(|w| w.name.as_str()).unwrap_or("");
                let title = who.filter(|_| recording.is_some()).map(|w| w.title.as_str()).unwrap_or("");
                Piece::Mention { text: shown(name, &unnamed(index), title), live: true }
            }
        })
        .collect()
}

pub fn to_model(text: &str, speakers: &[Who]) -> String {
    let mut out = String::new();
    for t in tokens(text, speakers) {
        match t {
            Tok::Text(s) => out.push_str(s),
            Tok::Speaker { index, recording: Some(r), .. } => out.push_str(&folder_label(index, r)),
            Tok::Speaker { index, recording: None, .. } => out.push_str(&label(index)),
        }
    }
    out
}

pub fn tag_recording(text: &str, recording: u32) -> String {
    let mut out = String::new();
    for t in tokens(text, &[]) {
        match t {
            Tok::Text(s) => out.push_str(s),
            Tok::Speaker { index, recording: Some(r), .. } => out.push_str(&folder_label(index, r)),
            Tok::Speaker { index, recording: None, .. } => out.push_str(&folder_label(index, recording)),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub at: usize,
    pub query: String,
}

fn char_floor(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    (0..=at).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0)
}

pub fn mention_query(text: &str, cursor: usize) -> Option<Query> {
    let cursor = char_floor(text, cursor);
    let head = &text[..cursor];
    let rel = head.rfind('@')?;
    if !boundary_before(text, rel) {
        return None;
    }
    let query = &text[rel + '@'.len_utf8()..cursor];
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    Some(Query { at: rel, query: query.to_string() })
}

pub fn matching(speakers: &[Who], query: &str, unnamed: impl Fn(u32) -> String) -> Vec<usize> {
    let q = query.trim().trim_start_matches('@').to_lowercase();
    speakers
        .iter()
        .enumerate()
        .filter(|(_, w)| {
            if q.is_empty() {
                return true;
            }
            let name = if w.name.trim().is_empty() { unnamed(w.index) } else { w.name.trim().to_string() };
            let shown = if w.recording.is_some() && !w.title.is_empty() { format!("{name} ({})", w.title) } else { name.clone() };
            let lab = match w.recording {
                Some(r) => folder_label(w.index, r),
                None => label(w.index),
            };
            shown.to_lowercase().contains(&q) || name.to_lowercase().contains(&q) || lab.trim_start_matches('@').to_lowercase().contains(&q)
        })
        .map(|(i, _)| i)
        .collect()
}

pub fn insert(text: &str, at: usize, query: &str, insertion: &str) -> (String, usize) {
    let end = (at + '@'.len_utf8() + query.len()).min(text.len());
    let rest = &text[end..];
    let mut out = String::new();
    out.push_str(&text[..at.min(text.len())]);
    out.push_str(insertion);
    let caret = if insertion.ends_with(char::is_whitespace) || rest.starts_with(char::is_whitespace) {
        out.len()
    } else {
        out.push(' ');
        out.len()
    };
    out.push_str(rest);
    (out, caret)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Popup {
    pub at: usize,
    pub selected: usize,
    pub dismissed: Option<usize>,
}

impl Default for Popup {
    fn default() -> Self {
        Self { at: usize::MAX, selected: 0, dismissed: None }
    }
}

impl Popup {
    pub fn is_open(&self, query: Option<&Query>, count: usize) -> bool {
        matches!(query, Some(q) if count > 0 && self.dismissed != Some(q.at))
    }

    pub fn update(&mut self, query: Option<&Query>, count: usize) {
        let Some(q) = query.filter(|q| count > 0 && self.dismissed != Some(q.at)) else {
            return;
        };
        if self.at != q.at {
            self.selected = 0;
            self.at = q.at;
        }
        self.selected = self.selected.min(count - 1);
    }

    pub fn up(&mut self, count: usize) {
        if count > 0 {
            self.selected = (self.selected + count - 1) % count;
        }
    }

    pub fn down(&mut self, count: usize) {
        if count > 0 {
            self.selected = (self.selected + 1) % count;
        }
    }

    pub fn dismiss(&mut self, at: usize) {
        self.dismissed = Some(at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn who(index: u32, name: &str) -> Who {
        Who { index, recording: None, name: name.into(), title: String::new() }
    }

    fn folder(index: u32, recording: u32, name: &str, title: &str) -> Who {
        Who { index, recording: Some(recording), name: name.into(), title: title.into() }
    }

    fn unnamed(index: u32) -> String {
        format!("Говорящий {}", index + 1)
    }

    #[test]
    fn labels_parse_both_ways() {
        assert_eq!(label(0), "@s1");
        assert_eq!(label(11), "@s12");
        assert_eq!(folder_label(1, 3), "@s2#3");
        assert_eq!(parse_label("@s1"), Some(Parsed { index: 0, recording: None }));
        assert_eq!(parse_label("s2"), Some(Parsed { index: 1, recording: None }));
        assert_eq!(parse_label("@s2#3"), Some(Parsed { index: 1, recording: Some(3) }));
        assert_eq!(parse_label("s2#3"), Some(Parsed { index: 1, recording: Some(3) }));
        assert_eq!(parse_label("@s0"), None);
        assert_eq!(parse_label("@s1#0"), None);
        assert_eq!(parse_label("Говорящий 1"), None);
        assert_eq!(parse_label("Антон"), None);
        assert_eq!(parse_label("1"), None);
    }

    #[test]
    fn names_become_labels_and_back() {
        let speakers = vec![who(0, "Антон"), who(1, "")];
        assert_eq!(to_model("спроси @Антон и @Говорящий 2", &speakers), "спроси @s1 и @s2");
        assert_eq!(to_model("ask @Speaker 2 please", &speakers), "ask @s2 please");
        assert_eq!(to_model("@антон, @s1", &speakers), "@s1, @s1");
        assert_eq!(to_model("почта a@anton.ru и @s1", &speakers), "почта a@anton.ru и @s1");
        let many = vec![who(0, ""), who(11, "")];
        assert_eq!(to_model("@Говорящий 12", &many), "@s12");
        assert_eq!(to_model("@Говорящий 1", &many), "@s1");
        let shown: Vec<_> = pieces("сказал @s1 и @s2", &speakers, unnamed).into_iter().map(|p| match p {
            Piece::Text(s) => s.to_string(),
            Piece::Mention { text, live } => format!("{text}:{live}"),
        }).collect();
        assert_eq!(shown, vec!["сказал ".to_string(), "@Антон:true".to_string(), " и ".to_string(), "@Говорящий 2:true".to_string()]);
    }

    #[test]
    fn a_rename_changes_only_the_live_label() {
        let before = vec![who(0, "Антон")];
        assert_eq!(to_model("привет @Антон", &before), "привет @s1");
        let after = vec![who(0, "Олег")];
        let live = pieces("привет @s1", &after, unnamed);
        assert_eq!(live, vec![Piece::Text("привет "), Piece::Mention { text: "@Олег".into(), live: true }]);
        let typed = pieces("привет @Антон", &after, unnamed);
        assert_eq!(typed, vec![Piece::Text("привет @Антон")]);
    }

    #[test]
    fn folder_labels_need_the_recording() {
        let speakers = vec![folder(1, 3, "Анна", "Планёрка"), folder(1, 4, "Анна", "Стендап"), folder(0, 3, "", "Планёрка")];
        assert_eq!(to_model("@s2#3 сказала", &speakers), "@s2#3 сказала");
        assert_eq!(to_model("@Анна", &speakers), "@Анна");
        assert_eq!(tag_recording("[#3 0:10] @s2: да\n@s1#9 уже", 3), "[#3 0:10] @s2#3: да\n@s1#9 уже");
        let live = pieces("см. @s2#3 и @s1#3", &speakers, unnamed);
        assert_eq!(
            live,
            vec![
                Piece::Text("см. "),
                Piece::Mention { text: "@Анна (Планёрка)".into(), live: true },
                Piece::Text(" и "),
                Piece::Mention { text: "@Говорящий 1 (Планёрка)".into(), live: true },
            ]
        );
    }

    #[test]
    fn completion_inserts_and_moves_with_the_caret() {
        let speakers = [who(0, "Антон"), who(1, "Анна"), folder(0, 3, "Борис", "Планёрка")];
        let typed = "привет @ан";
        let q = mention_query(typed, typed.len()).unwrap();
        assert_eq!((q.at, q.query.as_str()), (typed.find('@').unwrap(), "ан"));
        assert!(mention_query("a@anton.ru", 8).is_none());
        assert!(mention_query("привет @ан тон", "привет @ан тон".len()).is_none());
        let rows = matching(&speakers[..2], "ан", unnamed);
        assert_eq!(rows, vec![0, 1]);
        assert_eq!(matching(&speakers[..2], "", unnamed), vec![0, 1]);
        assert_eq!(matching(&speakers[2..], "план", unnamed), vec![0]);
        assert_eq!(matching(&speakers[2..], "s1#3", unnamed), vec![0]);
        let (text, caret) = insert("привет @ан!", 13, "ан", "@Антон");
        assert_eq!(text, "привет @Антон !");
        assert_eq!(caret, "привет @Антон ".len());
        let (text, _) = insert("привет @s", 13, "s", "@s1#3");
        assert_eq!(text, "привет @s1#3 ");

        let mut popup = Popup::default();
        let q = mention_query("привет @", "привет @".len()).unwrap();
        popup.update(Some(&q), 2);
        assert!(popup.is_open(Some(&q), 2));
        popup.down(2);
        assert_eq!(popup.selected, 1);
        popup.up(2);
        assert_eq!(popup.selected, 0);
        popup.dismiss(q.at);
        assert!(!popup.is_open(Some(&q), 2));
        let q2 = mention_query("ещё @", "ещё @".len()).unwrap();
        popup.update(Some(&q2), 2);
        assert!(popup.is_open(Some(&q2), 2));
        assert_eq!(popup.selected, 0);
    }
}
