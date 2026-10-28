use mediaa_core::agent::{WebEngine, WebSettings};
use serde_json::Value;
use std::io::Read;
use std::time::Duration;

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36";
const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_PAGE: u64 = 4 << 20;

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct WebHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub title: String,
    pub url: String,
    pub text: String,
}

fn agent(web: &WebSettings) -> anyhow::Result<ureq::Agent> {
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(TIMEOUT))
        .http_status_as_error(false);
    match web.proxy.trim() {
        "" => {}
        url => {
            let proxy = ureq::Proxy::new(url)
                .map_err(|error| anyhow::anyhow!("proxy \"{url}\": {error}"))?;
            builder = builder.proxy(Some(proxy));
        }
    }
    Ok(builder.build().new_agent())
}

struct Fetched {
    status: u16,
    content_type: String,
    body: Vec<u8>,
}

impl Fetched {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    fn json(&self) -> anyhow::Result<Value> {
        Ok(serde_json::from_slice(&self.body)?)
    }
}

fn take(response: ureq::http::Response<ureq::Body>) -> anyhow::Result<Fetched> {
    let status = response.status().as_u16();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let mut response = response;
    let mut body = Vec::new();
    response
        .body_mut()
        .as_reader()
        .take(MAX_PAGE)
        .read_to_end(&mut body)?;
    Ok(Fetched {
        status,
        content_type,
        body,
    })
}

fn text_of(e: scraper::ElementRef) -> String {
    e.text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn unwrap_redirect(href: &str) -> Option<String> {
    let full = if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_string()
    };
    let url = url::Url::parse(&full).ok()?;
    if url
        .host_str()
        .is_some_and(|h| h.ends_with("duckduckgo.com"))
    {
        return url
            .query_pairs()
            .find(|(k, _)| k == "uddg")
            .map(|(_, v)| v.into_owned());
    }
    Some(full)
}

pub fn parse_duckduckgo(html: &str) -> anyhow::Result<Vec<WebHit>> {
    let doc = scraper::Html::parse_document(html);
    let result = scraper::Selector::parse("div.result").expect("selector");
    let link = scraper::Selector::parse("a.result__a").expect("selector");
    let snippet = scraper::Selector::parse(".result__snippet").expect("selector");
    let mut hits = Vec::new();
    for r in doc.select(&result) {
        if r.value().classes().any(|c| c == "result--ad") {
            continue;
        }
        let Some(a) = r.select(&link).next() else {
            continue;
        };
        let Some(url) = a.value().attr("href").and_then(unwrap_redirect) else {
            continue;
        };
        hits.push(WebHit {
            title: text_of(a),
            url,
            snippet: r.select(&snippet).next().map(text_of).unwrap_or_default(),
        });
    }
    if hits.is_empty() && (html.contains("anomaly-modal") || html.contains("challenge-form")) {
        anyhow::bail!("DuckDuckGo asks to prove a human is searching; choose SearXNG or a search key (Agent → Providers)");
    }
    Ok(hits)
}

fn parse_json(body: &Value, items: &[&str], fields: (&str, &str, &str)) -> Vec<WebHit> {
    let mut list = body;
    for k in items {
        list = &list[*k];
    }
    list.as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    Some(WebHit {
                        title: r[fields.0].as_str().unwrap_or_default().trim().to_string(),
                        url: r[fields.1].as_str()?.trim().to_string(),
                        snippet: r[fields.2].as_str().unwrap_or_default().trim().to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn ok(fetched: Fetched, what: &str) -> anyhow::Result<Fetched> {
    match fetched.status {
        200..=299 => Ok(fetched),
        403 if fetched.content_type.starts_with("text/html") => {
            anyhow::bail!("{what} refuses requests from this network (403); set a proxy below or pick another search service")
        }
        401 | 403 => anyhow::bail!("{what}: the key was rejected ({})", fetched.status),
        429 => anyhow::bail!("{what}: too many requests, try later"),
        status => anyhow::bail!("{what}: {status}"),
    }
}

fn get(agent: &ureq::Agent, url: &str, headers: &[(&str, &str)]) -> anyhow::Result<Fetched> {
    let mut request = agent.get(url).header("user-agent", USER_AGENT);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    take(request.call()?)
}

pub fn search(web: &WebSettings, query: &str, n: usize) -> anyhow::Result<Vec<WebHit>> {
    let c = agent(web)?;
    let key = web.api_key.trim();
    let mut hits = match web.engine {
        WebEngine::DuckDuckGo => {
            let mut url = url::Url::parse("https://html.duckduckgo.com/html/")?;
            url.query_pairs_mut().append_pair("q", query);
            let resp = ok(get(&c, url.as_str(), &[])?, "DuckDuckGo")?;
            parse_duckduckgo(&resp.text())?
        }
        WebEngine::SearXng => {
            let base = web.searxng_url.trim().trim_end_matches('/');
            anyhow::ensure!(!base.is_empty(), "no SearXNG address (Agent → Providers)");
            let mut url = url::Url::parse(&format!("{base}/search"))?;
            url.query_pairs_mut()
                .append_pair("q", query)
                .append_pair("format", "json");
            let resp = ok(get(&c, url.as_str(), &[])?, "SearXNG")?;
            parse_json(&resp.json()?, &["results"], ("title", "url", "content"))
        }
        WebEngine::Brave => {
            anyhow::ensure!(!key.is_empty(), "no Brave Search key (Agent → Providers)");
            let mut url = url::Url::parse("https://api.search.brave.com/res/v1/web/search")?;
            url.query_pairs_mut()
                .append_pair("q", query)
                .append_pair("count", &n.to_string());
            let resp = ok(
                get(&c, url.as_str(), &[("X-Subscription-Token", key)])?,
                "Brave Search",
            )?;
            parse_json(
                &resp.json()?,
                &["web", "results"],
                ("title", "url", "description"),
            )
        }
        WebEngine::Tavily => {
            anyhow::ensure!(!key.is_empty(), "no Tavily key (Agent → Providers)");
            let body = serde_json::json!({"query": query, "max_results": n});
            let response = c
                .post("https://api.tavily.com/search")
                .header("user-agent", USER_AGENT)
                .header("Authorization", &format!("Bearer {key}"))
                .send_json(&body)?;
            parse_json(
                &ok(take(response)?, "Tavily")?.json()?,
                &["results"],
                ("title", "url", "content"),
            )
        }
    };
    hits.truncate(n);
    Ok(hits)
}

pub fn readable(html: &str, url: &str) -> anyhow::Result<(String, String)> {
    let cfg = dom_smoothie::Config {
        text_mode: dom_smoothie::TextMode::Markdown,
        ..Default::default()
    };
    let mut r = dom_smoothie::Readability::new(html, Some(url), Some(cfg))
        .map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
    let article = r
        .parse()
        .map_err(|e| anyhow::anyhow!("{url}: no readable text ({e})"))?;
    Ok((article.title.to_string(), article.text_content.to_string()))
}

pub fn lean(markdown: &str) -> String {
    static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"!?\[((?:[^\[\]]|\[[^\]]*\])*)\]\([^)]*\)").expect("regex")
    });
    static FOOTNOTE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^\[\d+\]$").expect("regex"));
    let text = LINK.replace_all(markdown, |c: &regex::Captures| {
        if c[0].starts_with('!') || FOOTNOTE.is_match(&c[1]) {
            String::new()
        } else {
            c[1].to_string()
        }
    });
    let text = text.replace("<br>", " ");
    let text: Vec<&str> = text
        .lines()
        .filter(|l| l.trim().is_empty() || !l.chars().all(|c| matches!(c, '|' | '-' | ' ')))
        .collect();
    let text = text.join("\n");
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, chars.peek()) {
            ('\\', Some(n)) if n.is_ascii_punctuation() => out.push(chars.next().expect("peeked")),
            _ => out.push(c),
        }
    }
    out
}

pub fn read(web: &WebSettings, url: &str, max_chars: usize) -> anyhow::Result<Page> {
    let parsed = url::Url::parse(url.trim())
        .map_err(|e| anyhow::anyhow!("«{url}» is not an address: {e}"))?;
    anyhow::ensure!(
        matches!(parsed.scheme(), "http" | "https"),
        "only http(s) pages are read"
    );
    let resp = ok(
        get(&agent(web)?, parsed.as_str(), &[])?,
        parsed.host_str().unwrap_or("the page"),
    )?;
    let kind = resp.content_type.to_lowercase();
    anyhow::ensure!(
        kind.is_empty() || kind.contains("html") || kind.starts_with("text/"),
        "{url}: not a text page ({kind})"
    );
    let body = resp.text();
    let (title, text) = if kind.contains("html") || body.trim_start().starts_with('<') {
        let (title, markdown) = readable(&body, parsed.as_str())?;
        (title, lean(&markdown))
    } else {
        (String::new(), body)
    };
    let mut text: String = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if text.chars().count() > max_chars {
        text = text.chars().take(max_chars).collect::<String>() + "\n… (the page goes on)";
    }
    Ok(Page {
        title,
        url: parsed.to_string(),
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duckduckgo_results_without_ads() {
        let html = r#"<html><body>
            <div class="result result--ad"><a class="result__a" href="https://ads.example/x">Реклама</a></div>
            <div class="result results_links"><h2><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fhabr.com%2Fru%2Farticles%2F1%2F&amp;rut=abc">Хабр:  статья</a></h2>
              <a class="result__snippet" href="x">Про <b>трудовую</b> книжку.</a></div>
            <div class="result"><a class="result__a" href="https://example.org/page">Example</a></div>
        </body></html>"#;
        let hits = parse_duckduckgo(html).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0],
            WebHit {
                title: "Хабр: статья".into(),
                url: "https://habr.com/ru/articles/1/".into(),
                snippet: "Про трудовую книжку.".into()
            }
        );
        assert_eq!(hits[1].url, "https://example.org/page");
        assert!(parse_duckduckgo("<div id=\"anomaly-modal\"></div>")
            .unwrap_err()
            .to_string()
            .contains("SearXNG"));
        assert!(
            parse_duckduckgo("<html></html>").unwrap().is_empty(),
            "nothing found is not an error"
        );
    }

    #[test]
    fn api_results() {
        let brave = serde_json::json!({"web": {"results": [{"title": "A", "url": "https://a", "description": "d"}, {"title": "no url"}]}});
        assert_eq!(
            parse_json(&brave, &["web", "results"], ("title", "url", "description")),
            [WebHit {
                title: "A".into(),
                url: "https://a".into(),
                snippet: "d".into()
            }]
        );
        let searx =
            serde_json::json!({"results": [{"title": "S", "url": "https://s", "content": "c"}]});
        assert_eq!(
            parse_json(&searx, &["results"], ("title", "url", "content"))[0].snippet,
            "c"
        );
        assert!(parse_json(
            &serde_json::json!({}),
            &["results"],
            ("title", "url", "content")
        )
        .is_empty());
    }

    #[test]
    fn lean_markdown() {
        assert_eq!(
            lean(
                r#"См. [документ](https://ru.wikipedia.org/wiki/Документ "Документ") и ![фото](x.png)\. Список\[1\]"#
            ),
            "См. документ и . Список[1]"
        );
        assert_eq!(lean("| А | Б |\n| --- | --- |\n|  |\n| <br>Разворот |\nработу[[1]](#cite_note-1), [[x]](#y)."), "| А | Б |\n|  Разворот |\nработу, [x].");
    }

    #[test]
    fn readable_text_of_a_page() {
        let html = format!(
            "<html><head><title>Трудовая книжка</title></head><body><nav>Меню Меню Меню</nav><article><h1>Трудовая книжка</h1>{}</article><footer>© сайт</footer></body></html>",
            "<p>Электронная трудовая книжка ведётся в СФР с 2020 года, и работник может запросить выписку в любой момент через Госуслуги или у работодателя.</p>".repeat(6)
        );
        let (title, text) = readable(&html, "https://example.org/t").unwrap();
        assert_eq!(title, "Трудовая книжка");
        assert!(
            text.contains("Электронная трудовая книжка ведётся в СФР"),
            "{text}"
        );
        assert!(!text.contains("Меню Меню"), "navigation left out: {text}");
    }

    #[test]
    #[ignore]
    fn searches_and_reads_the_web() {
        let mut web = WebSettings::default();
        if let Ok(key) = std::env::var("MEDIAA_BENCH_TAVILY") {
            web.engine = WebEngine::Tavily;
            web.api_key = key;
        }
        if let Ok(proxy) = std::env::var("MEDIAA_BENCH_PROXY") {
            web.proxy = proxy;
        }
        let urls = match search(&web, "электронная трудовая книжка выписка", 5)
        {
            Ok(hits) => {
                eprintln!("{hits:#?}");
                assert!(!hits.is_empty());
                hits.into_iter().map(|h| h.url).collect()
            }
            Err(e) if e.to_string().contains("human") => {
                eprintln!("search skipped: {e}");
                vec!["https://ru.wikipedia.org/wiki/Трудовая_книжка".to_string()]
            }
            Err(e) => panic!("{e:#}"),
        };
        let page = urls
            .iter()
            .filter_map(|u| {
                read(&web, u, 3000)
                    .inspect_err(|e| eprintln!("{u}: {e:#}"))
                    .ok()
            })
            .find(|p| p.text.chars().count() > 200);
        let page = page.expect("a page with text");
        eprintln!("{} — {}\n{}", page.title, page.url, page.text);
    }
}
