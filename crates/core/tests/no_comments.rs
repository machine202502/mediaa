use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{Table, Value};
use walkdir::WalkDir;

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset].bytes().filter(|b| *b == b'\n').count() + 1
}

fn report(path: &Path, src: &str, offsets: &[usize], hits: &mut Vec<String>) {
    for &offset in offsets {
        let line = line_of(src, offset);
        let text = src[offset..].lines().next().unwrap_or("");
        hits.push(format!("{}:{line}: {text}", path.display()));
    }
}

fn line_number(src: &str, pos: usize) -> usize {
    src.as_bytes()[..pos.min(src.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

fn line_start_at(src: &str, pos: usize) -> usize {
    src[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

fn line_at(src: &str, pos: usize) -> &str {
    let start = line_start_at(src, pos);
    let end = src[start..].find('\n').map(|i| start + i).unwrap_or(src.len());
    &src[start..end]
}

fn prev_code_char(src: &str, comments: &[(usize, usize)], pos: usize) -> Option<u8> {
    let bytes = src.as_bytes();
    let mut i = pos;
    loop {
        while i > 0 && (bytes[i - 1] as char).is_ascii_whitespace() {
            i -= 1;
        }
        if i == 0 {
            return None;
        }
        if let Some(&(start, _)) = comments.iter().find(|(start, end)| *start < i && i <= *end) {
            i = start;
            continue;
        }
        return Some(bytes[i - 1]);
    }
}

fn next_code_char(src: &str, comments: &[(usize, usize)], pos: usize) -> Option<u8> {
    let bytes = src.as_bytes();
    let n = bytes.len();
    let mut i = pos;
    loop {
        while i < n && (bytes[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        if i >= n {
            return None;
        }
        if let Some(&(_, end)) = comments.iter().find(|(start, end)| *start <= i && i < *end) {
            i = end;
            continue;
        }
        return Some(bytes[i]);
    }
}

fn previous_nonempty_line(src: &str, pos: usize) -> Option<&str> {
    let lines: Vec<&str> = src.lines().collect();
    let mut line = line_number(src, pos);
    while line > 1 {
        line -= 1;
        let text = lines[line - 1];
        if !text.trim().is_empty() {
            return Some(text);
        }
    }
    None
}

fn next_nonempty_line(src: &str, pos: usize) -> Option<&str> {
    let mut line = line_number(src, pos);
    let lines: Vec<&str> = src.lines().collect();
    while line < lines.len() {
        line += 1;
        let text = lines[line - 1];
        if !text.trim().is_empty() {
            return Some(text);
        }
    }
    None
}

fn ignored_result_line(text: &str) -> bool {
    text.contains("let _ =")
        || text.contains(".ok();")
        || text.contains(".ok()")
        || text.contains(".unwrap_or_default()")
}

fn rust_comments(src: &str) -> Vec<usize> {
    let mut comments: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    for token in rustc_lexer::tokenize(src) {
        let start = at;
        at += token.len;
        if matches!(
            token.kind,
            rustc_lexer::TokenKind::LineComment | rustc_lexer::TokenKind::BlockComment { .. }
        ) {
            comments.push((start, at));
        }
    }
    let lines: Vec<usize> = comments.iter().map(|(start, _)| line_number(src, *start)).collect();
    let mut hits = Vec::new();
    for (index, &(start, end)) in comments.iter().enumerate() {
        let text = &src[start..end];
        let trimmed = text.trim_start();
        let doc = trimmed.starts_with("///") || trimmed.starts_with("//!") || trimmed.starts_with("/**");
        if !doc && prev_code_char(src, &comments, start) == Some(b'{') && next_code_char(src, &comments, end) == Some(b'}') {
            continue;
        }
        if !doc && (ignored_result_line(&src[line_start_at(src, start)..start])
            || previous_nonempty_line(src, start).is_some_and(ignored_result_line)
            || next_nonempty_line(src, end).is_some_and(ignored_result_line))
        {
            continue;
        }
        let mut first = index;
        while first > 0
            && src[comments[first - 1].0..comments[first - 1].1].starts_with("//")
            && lines[first - 1] + 1 == lines[first]
        {
            first -= 1;
        }
        let mut last = index;
        while last + 1 < comments.len()
            && src[comments[last + 1].0..comments[last + 1].1].starts_with("//")
            && lines[last] + 1 == lines[last + 1]
        {
            last += 1;
        }
        let run: Vec<&str> = (first..=last).map(|i| &src[comments[i].0..comments[i].1]).collect();
        if run.iter().any(|line| line.trim_start().starts_with("// SAFETY")) {
            let after = comments[last].1;
            let n = src.len();
            let mut i = after;
            while i < n && (src.as_bytes()[i] as char).is_ascii_whitespace() {
                i += 1;
            }
            if i < n && line_at(src, i).contains("unsafe") {
                continue;
            }
        }
        if doc && run.iter().all(|line| line.starts_with("///")) {
            let has_safety = run
                .iter()
                .any(|line| line.trim_start_matches("///").trim_start().starts_with("# Safety"));
            if has_safety {
                let mut line = lines[last] + 1;
                let all: Vec<&str> = src.lines().collect();
                while line <= all.len() && all[line - 1].trim_start().starts_with("#[") {
                    line += 1;
                }
                if line <= all.len() && (all[line - 1].contains("unsafe fn") || all[line - 1].contains("unsafe trait")) {
                    continue;
                }
            }
        }
        hits.push(start);
    }
    hits
}

fn j2_comments(src: &str) -> Vec<usize> {
    src.match_indices("{#").map(|(at, _)| at).collect()
}

fn js_allowed(prev: u8, word: &str) -> bool {
    matches!(
        word,
        "return"
            | "typeof"
            | "instanceof"
            | "in"
            | "of"
            | "new"
            | "delete"
            | "void"
            | "throw"
            | "case"
            | "do"
            | "else"
            | "yield"
            | "await"
    ) || prev == 0
        || b"([{,;:=!&|?+-*%<>~^".contains(&prev)
}

fn js_string(bytes: &[u8], start: usize, quote: u8) -> usize {
    let mut i = start + 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == quote {
            return i + 1;
        } else if bytes[i] == b'\n' {
            return i;
        } else {
            i += 1;
        }
    }
    i
}

fn js_regex(bytes: &[u8], start: usize) -> usize {
    let mut i = start + 1;
    let mut class = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => {
                i += 2;
                continue;
            }
            b'[' => class = true,
            b']' => class = false,
            b'/' if !class => {
                i += 1;
                while i < bytes.len() && (bytes[i] as char).is_ascii_alphabetic() {
                    i += 1;
                }
                return i;
            }
            b'\n' => return i,
            _ => {}
        }
        i += 1;
    }
    i
}

fn js_ident(bytes: &[u8], mut i: usize) -> (usize, String) {
    let start = i;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
            i += 1;
        } else {
            break;
        }
    }
    (i, String::from_utf8_lossy(&bytes[start..i]).into_owned())
}

fn js_expr(bytes: &[u8], start: usize, out: &mut Vec<(usize, usize)>) -> usize {
    let n = bytes.len();
    let mut i = start;
    let mut depth = 1;
    let mut prev: u8 = 0;
    let mut word = String::new();
    while i < n {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_alphanumeric() || c == b'_' || c == b'$' {
            if c.is_ascii_digit() {
                while i < n && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_') {
                    i += 1;
                }
                prev = bytes[i - 1];
                word.clear();
            } else {
                let (next, ident) = js_ident(bytes, i);
                i = next;
                prev = bytes[i - 1];
                word = ident;
            }
            continue;
        }
        if c == b'/' && i + 1 < n && bytes[i + 1] != b'/' && bytes[i + 1] != b'*' && js_allowed(prev, &word) {
            i = js_regex(bytes, i);
            prev = b'/';
            word.clear();
            continue;
        }
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
            let start = i;
            i += 2;
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            out.push((start, i));
            continue;
        }
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            out.push((start, i));
            continue;
        }
        if c == b'\'' || c == b'"' {
            i = js_string(bytes, i, c);
            prev = c;
            word.clear();
            continue;
        }
        if c == b'`' {
            i = js_template(bytes, i, out);
            prev = b'`';
            word.clear();
            continue;
        }
        if c == b'{' {
            depth += 1;
        } else if c == b'}' {
            depth -= 1;
            if depth == 0 {
                return i + 1;
            }
        }
        prev = c;
        word.clear();
        i += 1;
    }
    i
}

fn js_template(bytes: &[u8], start: usize, out: &mut Vec<(usize, usize)>) -> usize {
    let n = bytes.len();
    let mut i = start + 1;
    while i < n {
        if bytes[i] == b'\\' {
            i += 2;
        } else if bytes[i] == b'`' {
            return i + 1;
        } else if bytes[i] == b'$' && i + 1 < n && bytes[i + 1] == b'{' {
            i = js_expr(bytes, i + 2, out);
        } else {
            i += 1;
        }
    }
    i
}

fn js_comments(src: &str) -> Vec<usize> {
    let bytes = src.as_bytes();
    let n = bytes.len();
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    let mut prev: u8 = 0;
    let mut word = String::new();
    while i < n {
        let c = bytes[i];
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'/' {
            let start = i;
            i += 2;
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            out.push((start, i));
            continue;
        }
        if c == b'/' && i + 1 < n && bytes[i + 1] == b'*' {
            let start = i;
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            out.push((start, i));
            continue;
        }
        if c == b'\'' || c == b'"' {
            i = js_string(bytes, i, c);
            prev = c;
            word.clear();
            continue;
        }
        if c == b'`' {
            i = js_template(bytes, i, &mut out);
            prev = b'`';
            word.clear();
            continue;
        }
        if c == b'/' {
            if js_allowed(prev, &word) {
                i = js_regex(bytes, i);
                prev = b'/';
            } else {
                prev = b'/';
                i += 1;
            }
            word.clear();
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' || c == b'$' {
            let (next, ident) = js_ident(bytes, i);
            i = next;
            prev = bytes[i - 1];
            word = ident;
            continue;
        }
        if c.is_ascii_digit() {
            while i < n && ((bytes[i] as char).is_ascii_alphanumeric() || bytes[i] == b'.' || bytes[i] == b'_') {
                i += 1;
            }
            prev = bytes[i - 1];
            word.clear();
            continue;
        }
        prev = c;
        word.clear();
        i += 1;
    }
    let comments = out;
    comments
        .iter()
        .filter(|(start, end)| {
            let body = src[*start..*end]
                .strip_prefix("//")
                .or_else(|| src[*start..*end].strip_prefix("/*"))
                .unwrap_or(&src[*start..*end])
                .trim_start();
            if body.starts_with("eslint-") || body.starts_with("@ts-") || body.starts_with("biome-ignore") {
                return false;
            }
            !(js_code_prev(bytes, &comments, *start) == Some(b'{') && js_code_next(bytes, &comments, *end) == Some(b'}'))
        })
        .map(|(start, _)| *start)
        .collect()
}

fn js_code_prev(bytes: &[u8], comments: &[(usize, usize)], pos: usize) -> Option<u8> {
    let mut i = pos;
    loop {
        while i > 0 && (bytes[i - 1] as char).is_ascii_whitespace() {
            i -= 1;
        }
        if i == 0 {
            return None;
        }
        if let Some(&(start, _)) = comments.iter().find(|(start, end)| *start < i && i <= *end) {
            i = start;
            continue;
        }
        return Some(bytes[i - 1]);
    }
}

fn js_code_next(bytes: &[u8], comments: &[(usize, usize)], pos: usize) -> Option<u8> {
    let n = bytes.len();
    let mut i = pos;
    loop {
        while i < n && (bytes[i] as char).is_ascii_whitespace() {
            i += 1;
        }
        if i >= n {
            return None;
        }
        if let Some(&(_, end)) = comments.iter().find(|(start, end)| *start <= i && i < *end) {
            i = end;
            continue;
        }
        return Some(bytes[i]);
    }
}

fn sh_heredoc(bytes: &[u8], start: usize, line_end: usize) -> Option<(String, bool, usize)> {
    let mut k = start + 2;
    let mut strip_tabs = false;
    if k < line_end && bytes[k] == b'-' {
        strip_tabs = true;
        k += 1;
    }
    while k < line_end && (bytes[k] == b' ' || bytes[k] == b'\t') {
        k += 1;
    }
    let delim;
    if k < line_end && (bytes[k] == b'\'' || bytes[k] == b'"') {
        let quote = bytes[k];
        k += 1;
        let s = k;
        while k < line_end && bytes[k] != quote {
            k += 1;
        }
        delim = String::from_utf8_lossy(&bytes[s..k]).into_owned();
        k += 1;
    } else {
        if k < line_end && bytes[k] == b'\\' {
            k += 1;
        }
        let s = k;
        while k < line_end && (bytes[k].is_ascii_alphanumeric() || bytes[k] == b'_') {
            k += 1;
        }
        delim = String::from_utf8_lossy(&bytes[s..k]).into_owned();
    }
    if delim.is_empty() {
        None
    } else {
        Some((delim, strip_tabs, k))
    }
}

fn sh_comments(src: &str) -> Vec<usize> {
    let bytes = src.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut quote: Option<u8> = None;
    let mut pending: Vec<(String, bool)> = Vec::new();
    let mut i = 0;
    let mut first = true;
    while i < n {
        let line_start = i;
        let line_end = src[i..].find('\n').map(|at| i + at).unwrap_or(n);
        if let Some((delim, strip_tabs)) = pending.first() {
            let line = String::from_utf8_lossy(&bytes[line_start..line_end]);
            let cmp = if *strip_tabs { line.trim_start_matches('\t') } else { line.as_ref() };
            if cmp == delim.as_str() {
                pending.remove(0);
            }
            i = (line_end + 1).min(n);
            continue;
        }
        let shebang = first && src[line_start..].starts_with("#!");
        first = false;
        let mut j = line_start;
        let mut declared = None;
        while j < line_end {
            let c = bytes[j];
            if quote == Some(b'\'') {
                if c == b'\'' {
                    quote = None;
                }
                j += 1;
                continue;
            }
            if quote == Some(b'"') {
                if c == b'\\' {
                    j += 2;
                    continue;
                }
                if c == b'"' {
                    quote = None;
                }
                j += 1;
                continue;
            }
            if c == b'\\' {
                j += 2;
                continue;
            }
            if c == b'\'' {
                quote = Some(b'\'');
                j += 1;
                continue;
            }
            if c == b'"' {
                quote = Some(b'"');
                j += 1;
                continue;
            }
            if c == b'$' && j + 1 < line_end && bytes[j + 1] == b'\'' {
                quote = Some(b'\'');
                j += 2;
                continue;
            }
            if !shebang
                && c == b'#'
                && (j == line_start || b" \t;&|()<>".contains(&bytes[j - 1]))
            {
                let text = src[j..line_end].trim_start();
                let directive =
                    text.starts_with("# shellcheck") || text.starts_with("# noqa") || text.starts_with("# type: ignore");
                if !directive {
                    out.push(j);
                }
                break;
            }
            if c == b'<' && j + 1 < line_end && bytes[j + 1] == b'<' && (j + 2 >= line_end || bytes[j + 2] != b'<') {
                if let Some((delim, strip_tabs, next)) = sh_heredoc(bytes, j, line_end) {
                    declared = Some((delim, strip_tabs));
                    j = next;
                    continue;
                }
            }
            j += 1;
        }
        if let Some(d) = declared {
            pending.push(d);
        }
        if line_end == n {
            break;
        }
        i = line_end + 1;
    }
    out
}

fn ps_comments(src: &str) -> Vec<usize> {
    let bytes = src.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut quote: Option<u8> = None;
    let mut block = false;
    let mut i = 0;
    while i < n {
        if block {
            if i + 1 < n && bytes[i] == b'#' && bytes[i + 1] == b'>' {
                block = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }
        if quote == Some(b'\'') {
            if bytes[i] == b'\'' {
                if i + 1 < n && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                quote = None;
            }
            i += 1;
            continue;
        }
        if quote == Some(b'"') {
            if bytes[i] == b'`' {
                i += 2;
            } else {
                if bytes[i] == b'"' {
                    quote = None;
                }
                i += 1;
            }
            continue;
        }
        let c = bytes[i];
        if c == b'`' {
            i += 2;
            continue;
        }
        if c == b'<' && i + 1 < n && bytes[i + 1] == b'#' {
            block = true;
            out.push(i);
            i += 2;
            continue;
        }
        if c == b'\'' {
            quote = Some(b'\'');
            i += 1;
            continue;
        }
        if c == b'"' {
            quote = Some(b'"');
            i += 1;
            continue;
        }
        if c == b'#' {
            let line_start = src[..i].rfind('\n').map(|at| at + 1).unwrap_or(0);
            let token_start = i == line_start || b" \t;|&(){}, ".contains(&bytes[i - 1]);
            if token_start {
                out.push(i);
                while i < n && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
        }
        i += 1;
    }
    out
}

fn cmake_comments(src: &str) -> Vec<usize> {
    let bytes = src.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = bytes[i];
        if c == b'\\' {
            i += 2;
            continue;
        }
        if c == b'"' {
            i += 1;
            while i < n {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if c == b'[' {
            let mut k = i + 1;
            let mut eq = 0;
            while k < n && bytes[k] == b'=' {
                eq += 1;
                k += 1;
            }
            if k < n && bytes[k] == b'[' {
                let mut close = String::from("]");
                for _ in 0..eq {
                    close.push('=');
                }
                close.push(']');
                if let Some(at) = src[k + 1..].find(&close) {
                    i = k + 1 + at + close.len();
                    continue;
                }
                return out;
            }
        }
        if c == b'#' {
            out.push(i);
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    out
}

fn docker_comments(src: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut at = 0;
    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            out.push(at + (line.len() - trimmed.len()));
        }
        at += line.len();
    }
    out
}

fn check_raw(raw: &toml_edit::RawString, src: &str, path: &Path, hits: &mut Vec<String>) {
    if let Some(span) = raw.span() {
        let text = &src[span.clone()];
        if let Some((at, _)) = text.match_indices('#').next() {
            hits.push(format!(
                "{}:{}: comment",
                path.display(),
                line_of(src, span.start + at)
            ));
        }
    }
}

fn check_decor(decor: &toml_edit::Decor, src: &str, path: &Path, hits: &mut Vec<String>) {
    for raw in [decor.prefix(), decor.suffix()].into_iter().flatten() {
        check_raw(raw, src, path, hits);
    }
}

fn walk_value(value: &Value, src: &str, path: &Path, hits: &mut Vec<String>) {
    check_decor(value.decor(), src, path, hits);
    match value {
        Value::Array(array) => {
            check_decor(array.decor(), src, path, hits);
            for item in array.iter() {
                walk_value(item, src, path, hits);
            }
        }
        Value::InlineTable(table) => {
            check_decor(table.decor(), src, path, hits);
            for (_, item) in table.iter() {
                walk_value(item, src, path, hits);
            }
        }
        _ => {}
    }
}

fn walk_item(item: &toml_edit::Item, src: &str, path: &Path, hits: &mut Vec<String>) {
    match item {
        toml_edit::Item::Value(value) => walk_value(value, src, path, hits),
        toml_edit::Item::Table(table) => walk_table(table, src, path, hits),
        toml_edit::Item::ArrayOfTables(tables) => {
            for table in tables.iter() {
                walk_table(table, src, path, hits);
            }
        }
        toml_edit::Item::None => {}
    }
}

fn walk_table(table: &Table, src: &str, path: &Path, hits: &mut Vec<String>) {
    check_decor(table.decor(), src, path, hits);
    for (key, item) in table.iter() {
        if let Some((key, _)) = table.get_key_value(key) {
            check_decor(key.leaf_decor(), src, path, hits);
            check_decor(key.dotted_decor(), src, path, hits);
        }
        walk_item(item, src, path, hits);
    }
}

fn toml_comments(src: &str, path: &Path) -> Vec<String> {
    let mut hits = Vec::new();
    let doc: toml_edit::ImDocument<String> = src.parse().expect("valid TOML");
    walk_table(doc.as_table(), src, path, &mut hits);
    check_raw(doc.trailing(), src, path, &mut hits);
    hits
}

fn collect(dir: &Path, extensions: &[&str], files: &mut Vec<PathBuf>) {
    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            name != "target" && name != "node_modules" && name != ".git"
        })
        .flatten()
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let dockerfile = path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.starts_with("Dockerfile"))
            .unwrap_or(false);
        if extensions.contains(&extension) || dockerfile {
            files.push(path.to_path_buf());
        }
    }
}

#[test]
fn no_comments_in_the_repository() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut files = Vec::new();
    collect(&root.join("crates"), &["rs", "toml", "j2"], &mut files);
    collect(&root.join("scripts"), &["sh", "mjs", "js", "ps1"], &mut files);
    collect(&root.join("cmake"), &["cmake"], &mut files);
    files.push(root.join("Cargo.toml"));
    files.push(root.join(".cargo/config.toml"));
    files.sort();

    let mut hits = Vec::new();
    for path in files {
        let src = fs::read_to_string(&path).expect("read");
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match extension {
            "rs" => report(&path, &src, &rust_comments(&src), &mut hits),
            "j2" => report(&path, &src, &j2_comments(&src), &mut hits),
            "sh" => report(&path, &src, &sh_comments(&src), &mut hits),
            "mjs" | "js" => report(&path, &src, &js_comments(&src), &mut hits),
            "ps1" => report(&path, &src, &ps_comments(&src), &mut hits),
            "cmake" => report(&path, &src, &cmake_comments(&src), &mut hits),
            "toml" => hits.extend(toml_comments(&src, &path)),
            _ => {
                if path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.starts_with("Dockerfile"))
                    .unwrap_or(false)
                {
                    report(&path, &src, &docker_comments(&src), &mut hits);
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "comments in code:\n{}",
        hits.join("\n")
    );
}

#[test]
fn rust_safety_comment_before_unsafe_is_allowed() {
    assert!(rust_comments("fn f() {\n    // SAFETY: the buffer is valid.\n    unsafe { }\n}\n").is_empty());
}

#[test]
fn rust_safety_comment_without_unsafe_fails() {
    assert_eq!(rust_comments("// SAFETY: nothing here.\nlet x = 1;\n").len(), 1);
}

#[test]
fn rust_safety_doc_on_unsafe_fn_is_allowed() {
    assert!(rust_comments("/// # Safety\n/// `p` must be valid.\nunsafe fn f(p: *const u8) {}\n").is_empty());
}

#[test]
fn rust_safety_doc_on_safe_fn_fails() {
    assert_eq!(rust_comments("/// # Safety\n/// `p` must be valid.\nfn f(p: *const u8) {}\n").len(), 2);
}

#[test]
fn rust_empty_block_comment_is_allowed() {
    assert!(rust_comments("match x {\n    _ => {\n        // nothing to do\n    }\n}\n").is_empty());
}

#[test]
fn rust_comment_beside_code_in_block_fails() {
    assert_eq!(rust_comments("{\n    // why\n    let x = 1;\n}\n").len(), 1);
}

#[test]
fn rust_ignored_result_comment_is_allowed() {
    assert!(rust_comments("// the file may be gone\nlet _ = fs::remove_file(p);\n").is_empty());
    assert!(rust_comments("let value = fs::read(p).ok(); // optional\n").is_empty());
    assert!(rust_comments("// a missing entry means the default\nlet v = map.get(k).unwrap_or_default();\n").is_empty());
}

#[test]
fn rust_plain_comment_fails() {
    assert_eq!(rust_comments("// hello\nlet x = 1;\n").len(), 1);
}

#[test]
fn js_empty_catch_comment_is_allowed() {
    assert!(js_comments("try {} catch {\n    // already gone\n}\n").is_empty());
}

#[test]
fn js_comment_beside_code_in_block_fails() {
    assert_eq!(js_comments("function f() {\n    // why\n    return 1;\n}\n").len(), 1);
}

#[test]
fn js_tool_directives_are_allowed() {
    let src = "// eslint-disable-next-line no-console\n// @ts-expect-error\n// biome-ignore lint/suspicious/noExplicitAny: x\nconst x = 1;\n";
    assert!(js_comments(src).is_empty());
}

#[test]
fn shell_directives_are_allowed() {
    assert!(sh_comments("#!/bin/sh\n# shellcheck disable=SC2086\n# noqa\n# type: ignore\necho hi\n").is_empty());
}

#[test]
fn shell_plain_comment_fails() {
    assert_eq!(sh_comments("#!/bin/sh\n# a note\necho hi\n").len(), 1);
}

#[test]
fn jinja_comment_fails() {
    assert_eq!(j2_comments("{# x #}\n").len(), 1);
}

#[test]
fn toml_comment_fails() {
    assert_eq!(toml_comments("# x\n[t]\na = 1\n", Path::new("x.toml")).len(), 1);
}

#[test]
fn cmake_comment_fails() {
    assert_eq!(cmake_comments("# x\nset(A 1)\n").len(), 1);
}

#[test]
fn docker_comment_fails() {
    assert_eq!(docker_comments("# x\nFROM ubuntu\n").len(), 1);
}

#[test]
fn powershell_comment_fails() {
    assert_eq!(ps_comments("# x\n").len(), 1);
}
