use mediaa2_core::agent::{ModelSource, WebEngine, WebSettings};
use mediaa2_model_eval::{EmbedKind, EmbeddingsRequest, Engine};
use serde_json::Value;
use std::io::Read;
use std::sync::atomic::AtomicBool;
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
        anyhow::bail!("DuckDuckGo asks to prove a human is searching; choose SearXNG or a search key (Transcription → Budget → Tools)");
    }
    Ok(hits)
}

/// Mojeek html: no key, no captcha wall so far.
pub fn parse_mojeek(html: &str) -> Vec<WebHit> {
    let doc = scraper::Html::parse_document(html);
    let result = scraper::Selector::parse("ul.results-standard li").expect("selector");
    let link = scraper::Selector::parse("a").expect("selector");
    let snippet = scraper::Selector::parse("p.s").expect("selector");
    let mut hits = Vec::new();
    for li in doc.select(&result) {
        let Some(a) = li.select(&link).find(|a| {
            a.value()
                .attr("href")
                .is_some_and(|h| h.starts_with("http"))
        }) else {
            continue;
        };
        let url = a.value().attr("href").unwrap_or_default().to_string();
        if url.contains("mojeek.com") {
            continue;
        }
        hits.push(WebHit {
            title: text_of(a),
            url,
            snippet: li.select(&snippet).next().map(text_of).unwrap_or_default(),
        });
    }
    hits
}

/// Bing html: no key; the layout changes now and then.
pub fn parse_bing(html: &str) -> Vec<WebHit> {
    let doc = scraper::Html::parse_document(html);
    let result = scraper::Selector::parse("li.b_algo").expect("selector");
    let link = scraper::Selector::parse("h2 a").expect("selector");
    let snippet = scraper::Selector::parse(".b_caption p, p").expect("selector");
    let mut hits = Vec::new();
    for li in doc.select(&result) {
        let Some(a) = li.select(&link).next() else {
            continue;
        };
        let Some(url) = a.value().attr("href") else {
            continue;
        };
        if !url.starts_with("http") || url.contains("bing.com") {
            continue;
        }
        hits.push(WebHit {
            title: text_of(a),
            url: url.to_string(),
            snippet: li.select(&snippet).next().map(text_of).unwrap_or_default(),
        });
    }
    hits
}

/// One of the keyless search engines and the request that runs it.
type FreeSearch = (
    &'static str,
    fn(&ureq::Agent, &WebSettings, &str, usize) -> anyhow::Result<Vec<WebHit>>,
);

/// Free search engines tried in turn: a captcha or an empty answer sends the
/// query to the next one.
fn search_free(
    c: &ureq::Agent,
    web: &WebSettings,
    query: &str,
    n: usize,
) -> anyhow::Result<Vec<WebHit>> {
    let attempts: [FreeSearch; 3] = [
        ("DuckDuckGo", search_ddg),
        ("Mojeek", search_mojeek),
        ("Bing", search_bing),
    ];
    let mut fallback: Option<anyhow::Result<Vec<WebHit>>> = None;
    for (name, run) in attempts {
        match run(c, web, query, n) {
            Ok(hits) if !hits.is_empty() => return Ok(hits),
            Ok(hits) => fallback = Some(Ok(hits)),
            Err(error) => {
                log::info!("web: {name} failed, trying the next free engine: {error:#}");
                fallback = Some(Err(error));
            }
        }
    }
    fallback.unwrap_or_else(|| Ok(Vec::new()))
}

fn search_ddg(
    c: &ureq::Agent,
    _web: &WebSettings,
    query: &str,
    _n: usize,
) -> anyhow::Result<Vec<WebHit>> {
    let mut url = url::Url::parse("https://html.duckduckgo.com/html/")?;
    url.query_pairs_mut().append_pair("q", query);
    let resp = ok(get(c, url.as_str(), &[])?, "DuckDuckGo")?;
    parse_duckduckgo(&resp.text())
}

fn search_mojeek(
    c: &ureq::Agent,
    _web: &WebSettings,
    query: &str,
    _n: usize,
) -> anyhow::Result<Vec<WebHit>> {
    let mut url = url::Url::parse("https://www.mojeek.com/search")?;
    url.query_pairs_mut().append_pair("q", query);
    let resp = ok(get(c, url.as_str(), &[])?, "Mojeek")?;
    let hits = parse_mojeek(&resp.text());
    if hits.is_empty() {
        anyhow::bail!("Mojeek returned no results");
    }
    Ok(hits)
}

fn search_bing(
    c: &ureq::Agent,
    web: &WebSettings,
    query: &str,
    _n: usize,
) -> anyhow::Result<Vec<WebHit>> {
    let mut url = url::Url::parse("https://www.bing.com/search")?;
    url.query_pairs_mut().append_pair("q", query);
    let _ = web;
    let resp = ok(get(c, url.as_str(), &[])?, "Bing")?;
    let hits = parse_bing(&resp.text());
    if hits.is_empty() {
        anyhow::bail!("Bing returned no results");
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
        WebEngine::DuckDuckGo => search_free(&c, web, query, n)?,
        WebEngine::SearXng => {
            let base = web.searxng_url.trim().trim_end_matches('/');
            anyhow::ensure!(
                !base.is_empty(),
                "no SearXNG address (Transcription → Budget → Tools)"
            );
            let mut url = url::Url::parse(&format!("{base}/search"))?;
            url.query_pairs_mut()
                .append_pair("q", query)
                .append_pair("format", "json");
            let resp = ok(get(&c, url.as_str(), &[])?, "SearXNG")?;
            parse_json(&resp.json()?, &["results"], ("title", "url", "content"))
        }
        WebEngine::Brave => {
            anyhow::ensure!(
                !key.is_empty(),
                "no Brave Search key (Transcription → Budget → Tools)"
            );
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
            anyhow::ensure!(
                !key.is_empty(),
                "no Tavily key (Transcription → Budget → Tools)"
            );
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

/// Keeps the head of a too-long page plus the paragraphs that match
/// `focus` (what the reader is looking for), instead of cutting the tail
/// blindly. Blocks stay in document order; skipped spans become "…".
/// With `embed`, the blocks are also ranked by meaning (the same embedding
/// model the recordings use).
pub fn smart_cut(
    text: &str,
    max_chars: usize,
    focus: &str,
    embed: Option<(&dyn Engine, &ModelSource)>,
) -> String {
    let total = text.chars().count();
    if total <= max_chars {
        return text.to_string();
    }
    let blocks: Vec<&str> = text
        .split("\n\n")
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .collect();
    if blocks.is_empty() {
        return text.chars().take(max_chars).collect::<String>();
    }
    let budget = max_chars.saturating_sub(16);
    let mut keep = vec![false; blocks.len()];
    // the opening paragraphs first: they carry the title and the lead
    let mut used = 0usize;
    for (i, block) in blocks.iter().enumerate() {
        let len = block.chars().count();
        if used + len > budget / 3 {
            break;
        }
        keep[i] = true;
        used += len;
    }
    // the closing paragraph: pages often end with the summary
    let last = blocks.len() - 1;
    if !keep[last] {
        let len = blocks[last].chars().count();
        if used + len <= budget {
            keep[last] = true;
            used += len;
        }
    }
    // then the blocks that answer the focus, best first
    let terms: Vec<String> = focus
        .to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '-'))
        .filter(|t| t.chars().count() > 2)
        .map(str::to_string)
        .collect();
    // a crude stem: Russian endings ("трудовая" → "трудов") still match
    let stems: Vec<String> = terms
        .iter()
        .map(|term| {
            let n = term.chars().count();
            let cut = if n > 4 { n - 2 } else { n - 1 };
            term.chars().take(cut).collect::<String>()
        })
        .filter(|stem| stem.chars().count() >= 3)
        .collect();
    let mut scored: Vec<(usize, f32)> = blocks
        .iter()
        .enumerate()
        .filter(|(i, _)| !keep[*i])
        .map(|(i, block)| {
            let lower = block.to_lowercase();
            let score = stems.iter().filter(|s| lower.contains(s.as_str())).count() as f32;
            (i, score)
        })
        .collect();
    if let Some((engine, source)) = embed {
        match block_cosines(engine, source, &blocks, focus) {
            Ok(cosines) => {
                for (i, cos) in cosines {
                    match scored.iter_mut().find(|(j, _)| *j == i) {
                        Some((_, score)) => *score += 3.0 * cos,
                        None => {
                            if cos > 0.35 {
                                scored.push((i, 3.0 * cos));
                            }
                        }
                    }
                }
            }
            Err(error) => log::info!("web: the page is cut by words only: {error:#}"),
        }
    }
    let mut scored: Vec<(usize, usize)> = scored
        .into_iter()
        .map(|(i, score)| (i, (score * 100.0) as usize))
        .filter(|(_, score)| *score > 0)
        .collect();
    scored.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    for (i, _) in scored {
        let len = blocks[i].chars().count();
        if used + len > budget {
            continue;
        }
        keep[i] = true;
        used += len;
    }
    let mut out = String::new();
    let mut previous_kept: Option<usize> = None;
    for (i, block) in blocks.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        if let Some(previous) = previous_kept {
            // adjacent blocks were neighbours in the original text
            out.push_str(if i > previous + 1 { "\n…\n" } else { "\n\n" });
        }
        previous_kept = Some(i);
        out.push_str(block);
    }
    if used < total {
        out.push_str("\n… (the page goes on)");
    }
    out
}

/// Ranks the page blocks against `focus` by meaning: the same embedding
/// model as the recordings use. Long paragraphs are trimmed for the request.
fn block_cosines(
    engine: &dyn Engine,
    source: &ModelSource,
    blocks: &[&str],
    focus: &str,
) -> anyhow::Result<Vec<(usize, f32)>> {
    let cancel = AtomicBool::new(false);
    let trim = |text: &str| text.chars().take(2000).collect::<String>();
    let request = |input: Vec<String>, kind: EmbedKind| EmbeddingsRequest {
        source: source.clone(),
        input,
        kind,
    };
    let focus_vec = engine
        .embeddings(&request(vec![trim(focus)], EmbedKind::Query), &cancel)?
        .into_iter()
        .next()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| anyhow::anyhow!("the model returned no embedding"))?;
    let mut out = Vec::new();
    for (batch_at, batch) in blocks.chunks(32).enumerate() {
        let vectors = engine.embeddings(
            &request(batch.iter().map(|b| trim(b)).collect(), EmbedKind::Document),
            &cancel,
        )?;
        for (j, vector) in vectors.iter().enumerate() {
            if vector.is_empty() {
                continue;
            }
            // the model vectors are unit-length, but do not rely on it
            let norm = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
            let (fa, fb) = (norm(&focus_vec), norm(vector));
            let cos = if fa > 0.0 && fb > 0.0 {
                crate::voices::cosine(&focus_vec, vector) / (fa * fb)
            } else {
                0.0
            };
            out.push((batch_at * 32 + j, cos));
        }
    }
    Ok(out)
}

/// Fetches a page and extracts its full readable text. The caller can cache
/// the result and re-cut it with [`smart_cut`] for different questions.
pub fn fetch_page(web: &WebSettings, url: &str) -> anyhow::Result<(String, String, String)> {
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
    let text: String = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok((parsed.to_string(), title, text))
}

pub fn read(
    web: &WebSettings,
    url: &str,
    max_chars: usize,
    focus: &str,
    embed: Option<(&dyn Engine, &ModelSource)>,
) -> anyhow::Result<Page> {
    let (url, title, text) = fetch_page(web, url)?;
    let text = smart_cut(&text, max_chars, focus, embed);
    Ok(Page { title, url, text })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_page_is_left_alone() {
        let text = "first paragraph.

second paragraph.";
        assert_eq!(smart_cut(text, 500, "никакой фокус", None), text);
    }

    #[test]
    fn a_long_page_keeps_the_head_the_tail_and_the_focused_blocks() {
        let mut blocks = vec!["Вступление про трудовые книжки.".to_string()];
        for i in 0..40 {
            blocks.push(format!(
                "Параграф {i} про погоду и сезоны. Слова погода, дождь."
            ));
        }
        blocks.push("12. Обновление трудовой книжки.".to_string());
        blocks.push("Заключение: трудовая книжка — главный документ.".to_string());
        let text = blocks.join(
            "

",
        );
        let cut = smart_cut(&text, 400, "трудовая книжка", None);
        println!(
            "CUT=
{cut}
END"
        );
        assert!(cut.chars().count() <= 420, "the cut respects the budget");
        assert!(cut.starts_with("Вступление"), "the head stays");
        assert!(cut.contains("Заключение"), "the closing summary stays");
        assert!(
            cut.contains("Обновление трудовой книжки"),
            "the block matching the focus stays"
        );
        assert!(cut.contains("…"), "the skipped span is marked");
        assert!(!cut.contains("Параграф 20"), "unrelated filler is dropped");
    }

    #[test]
    fn meaning_ranking_keeps_the_block_about_the_focus() {
        use mediaa2_core::agent::ModelSource;
        use mediaa2_model_eval::testing::FakeEngine;
        let engine = FakeEngine::new();
        let source = ModelSource::new("e5", "e5");
        let blocks = [
            "Head of the page.",
            "A long filler paragraph about many unrelated things, seasons, weather,              furniture and other topics that take a lot of space in this text.",
            "The bridge over the river.",
            "A shorter filler note about something else entirely.",
            "Tail summary.",
        ];
        let text = blocks.join(
            "

",
        );
        let cut = smart_cut(&text, 200, "bridge river", Some((&engine, &source)));
        assert!(
            engine.embeddings_count() > 0,
            "the blocks were ranked by meaning"
        );

        assert!(cut.contains("Head of the page."), "the head stays");
        assert!(
            cut.contains("The bridge over the river"),
            "the focused block stays"
        );
        assert!(
            cut.contains("A shorter filler note"),
            "the vector ranking saves the second filler over the long one"
        );
        assert!(
            !cut.contains("long filler paragraph"),
            "the long unrelated filler is dropped first"
        );
    }

    #[test]
    fn without_a_focus_a_long_page_degrades_to_head_and_tail() {
        let mut blocks = vec!["Head.".to_string()];
        for i in 0..60 {
            blocks.push(format!("Filler {i} about nothing in particular at all."));
        }
        blocks.push("Tail.".to_string());
        let text = blocks.join(
            "

",
        );
        let cut = smart_cut(&text, 200, "", None);
        assert!(cut.starts_with("Head."));
        assert!(cut.ends_with("… (the page goes on)"));
        assert!(cut.contains("Tail."));
    }

    #[test]
    fn mojeek_results_skip_the_site_itself() {
        let html = r#"<html><body>
            <ul class="results-standard">
              <li><h2><a href="https://mojeek.com/about">About Mojeek</a></h2><p class="s">self</p></li>
              <li><h2><a href="https://example.org/page">Example page</a></h2>
                <p class="s">About <b>examples</b> here.</p></li>
              <li><h2>No link here</h2></li>
            </ul>
        </body></html>"#;
        let hits = parse_mojeek(html);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.org/page");
        assert_eq!(hits[0].title, "Example page");
        assert_eq!(hits[0].snippet, "About examples here.");
    }

    #[test]
    fn bing_results_take_the_organic_list_only() {
        let html = r#"<html><body>
            <li class="b_algo"><h2><a href="https://example.org/a">Alpha</a></h2>
              <div class="b_caption"><p>First caption.</p></div></li>
            <li class="b_algo"><h2><a href="https://www.bing.com/x">Internal</a></h2></li>
            <li class="b_algo"><h2>No anchor</h2></li>
        </body></html>"#;
        let hits = parse_bing(html);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.org/a");
        assert_eq!(hits[0].snippet, "First caption.");
    }

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
                read(&web, u, 3000, "", None)
                    .inspect_err(|e| eprintln!("{u}: {e:#}"))
                    .ok()
            })
            .find(|p| p.text.chars().count() > 200);
        let page = page.expect("a page with text");
        eprintln!("{} — {}\n{}", page.title, page.url, page.text);
    }
}
