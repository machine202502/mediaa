//! The agent's way out to the web: searching and reading pages, for what the recordings do not
//! cover. Links in what the agent writes must lead to pages it was shown.

use crate::prompts;
use super::brain::ToolSpec;
use super::tools::Outcome;
use crate::web::{Page, WebHit};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::collections::HashSet;

/// Results of a web search.
const HITS: usize = 6;
/// A page with less text (characters) is a script's loader, not the page.
const EMPTY_PAGE: usize = 200;

pub type WebSearch<'a> = dyn Fn(&str, usize) -> anyhow::Result<Vec<WebHit>> + 'a;
pub type WebRead<'a> = dyn Fn(&str, usize) -> anyhow::Result<Page> + 'a;

/// Web search and page reading, with the addresses the agent has been shown.
pub struct Web<'a> {
    pub search: &'a WebSearch<'a>,
    pub read: &'a WebRead<'a>,
    /// Longest page text (characters): a page must leave room for what was found in the recordings.
    pub max_chars: usize,
    seen: RefCell<HashSet<String>>,
}

impl<'a> Web<'a> {
    pub fn new(search: &'a WebSearch<'a>, read: &'a WebRead<'a>, max_chars: usize) -> Web<'a> {
        Web { search, read, max_chars, seen: RefCell::default() }
    }

    pub fn specs() -> Vec<ToolSpec> {
        vec![
            ToolSpec {
                name: "web_search",
                description: prompts::tool("web_search"),
                params: json!({"type": "object", "properties": {"query": {"type": "string", "maxLength": 200}}, "required": ["query"]}),
                example: json!({"query": "<words to search for>"}),
            },
            ToolSpec {
                name: "web_read",
                description: prompts::tool("web_read"),
                params: json!({"type": "object", "properties": {"url": {"type": "string", "maxLength": 500}}, "required": ["url"]}),
                example: json!({"url": "<an address from web_search>"}),
            },
        ]
    }

    /// Runs a web tool (`None`: not one of them).
    pub fn run(&self, tool: &str, args: &Value) -> Option<anyhow::Result<Outcome>> {
        let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
        Some(match tool {
            "web_search" => self.search(&arg("query")),
            "web_read" => self.read(&arg("url")),
            _ => return None,
        })
    }

    fn search(&self, query: &str) -> anyhow::Result<Outcome> {
        anyhow::ensure!(!query.is_empty(), super::tools::needs("web_search", "query"));
        let hits = (self.search)(query, HITS)?;
        self.seen.borrow_mut().extend(hits.iter().map(|h| h.url.clone()));
        let text = prompts::render("results/web_search.j2", minijinja::context! { hits => &hits });
        Ok(Outcome { text, seen: Some(hits.into_iter().map(|h| h.url).collect()) })
    }

    fn read(&self, url: &str) -> anyhow::Result<Outcome> {
        anyhow::ensure!(!url.is_empty(), super::tools::needs("web_read", "address"));
        let page = (self.read)(url, self.max_chars)?;
        let mut seen = self.seen.borrow_mut();
        seen.insert(url.to_string());
        seen.insert(page.url.clone());
        // Some sites only send a script that loads the page: nothing to read there.
        let empty = page.text.chars().count() < EMPTY_PAGE;
        let text = prompts::render("results/web_page.j2", minijinja::context! { title => &page.title, url => &page.url, text => &page.text, empty });
        Ok(Outcome { text, seen: empty.then(Vec::new) })
    }

    /// `text` with links to pages the agent was not shown turned into their plain text.
    pub fn known_links(&self, text: &str) -> String {
        static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\[([^\]]*)\]\((https?://[^)\s]+)\)").expect("regex"));
        let seen = self.seen.borrow();
        LINK.replace_all(text, |c: &regex::Captures| if seen.contains(&c[2]) { c[0].to_string() } else { c[1].to_string() }).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hits(q: &str, _: usize) -> anyhow::Result<Vec<WebHit>> {
        Ok(match q {
            "трудовая книжка" => vec![WebHit { title: "Трудовая книжка".into(), url: "https://ru.wikipedia.org/wiki/Трудовая_книжка".into(), snippet: "Документ о трудовой деятельности.".into() }],
            _ => vec![],
        })
    }

    fn page(url: &str, max: usize) -> anyhow::Result<Page> {
        let text = if url.contains("loader") { "Сайт сейчас откроется".to_string() } else { "Текст. ".repeat(100) };
        Ok(Page { title: "Страница".into(), url: url.into(), text: text.chars().take(max).collect() })
    }

    #[test]
    fn searches_reads_and_keeps_only_shown_links() {
        let web = Web::new(&hits, &page, 300);
        let found = web.run("web_search", &json!({"query": "трудовая книжка"})).unwrap().unwrap();
        assert!(found.text.contains("https://ru.wikipedia.org/wiki/Трудовая_книжка") && found.text.contains("Документ"));
        let none = web.run("web_search", &json!({"query": "ничего"})).unwrap().unwrap();
        assert_eq!(none.seen, Some(vec![]));
        let read = web.run("web_read", &json!({"url": "https://example.org/a"})).unwrap().unwrap();
        assert!(read.text.starts_with("Страница — https://example.org/a") && read.text.chars().count() < 350 && read.seen.is_none());
        let loader = web.run("web_read", &json!({"url": "https://example.org/loader"})).unwrap().unwrap();
        assert!(loader.text.contains("Read another address") && loader.seen == Some(vec![]));
        assert!(web.run("web_search", &json!({})).unwrap().is_err());
        assert!(web.run("search", &json!({})).is_none());

        let text = "См. [вики](https://ru.wikipedia.org/wiki/Трудовая_книжка), [пример](https://example.org/a) и [выдумка](https://made.up/x) [1:00].";
        assert_eq!(web.known_links(text), "См. [вики](https://ru.wikipedia.org/wiki/Трудовая_книжка), [пример](https://example.org/a) и выдумка [1:00].");
    }
}
