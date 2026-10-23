use super::super::header;
use crate::app::App;
use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::{Provider, ProviderKind, WebEngine, WebSettings};
use rust_i18n::t;
use std::collections::HashMap;

enum CheckState {
    Running(Receiver<Result<Vec<String>, String>>),
    Done(Result<Vec<String>, String>),
}

#[derive(Default)]
pub struct ProvidersUi {
    checks: HashMap<String, CheckState>,
    shown_keys: Vec<String>,
    web_check: Option<CheckState>,
}

impl ProvidersUi {
    pub fn known_models(&self, id: &str) -> Option<&[String]> {
        match self.checks.get(id) {
            Some(CheckState::Done(Ok(m))) => Some(m),
            _ => None,
        }
    }

    pub fn start_check(&mut self, p: &Provider) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let p2 = p.clone();
        let _ = std::thread::Builder::new().name("provider-check".into()).spawn(move || {
            let _ = tx.send(mediaa_agent::providers::list_models(&p2).map_err(|e| format!("{e:#}")));
        });
        self.checks.insert(p.id.clone(), CheckState::Running(rx));
    }

    fn start_web_check(&mut self, web: &WebSettings) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let web = web.clone();
        let _ = std::thread::Builder::new().name("web-check".into()).spawn(move || {
            let found = mediaa_agent::web::search(&web, "MediaA", 5).map(|hits| hits.into_iter().map(|h| h.title).collect());
            let _ = tx.send(found.map_err(|e| format!("{e:#}")));
        });
        self.web_check = Some(CheckState::Running(rx));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        for c in self.checks.values_mut().chain(self.web_check.as_mut()) {
            if let CheckState::Running(rx) = c {
                match rx.try_recv() {
                    Ok(r) => *c = CheckState::Done(r),
                    Err(_) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                }
            }
        }
    }
}

fn kind_label(k: ProviderKind) -> String {
    match k {
        ProviderKind::OpenAiCompatible => t!("agent.kind_openai").into(),
        ProviderKind::OpenRouter => t!("agent.kind_openrouter").into(),
        ProviderKind::HuggingFace => "Hugging Face".into(),
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    app.agent.providers.poll(ui.ctx());
    header(ui, &t!("agent.providers_title"), "");
    ui.horizontal(|ui| {
        for kind in [ProviderKind::OpenAiCompatible, ProviderKind::OpenRouter, ProviderKind::HuggingFace] {
            if ui.add(theme::secondary(icon::PLUS, &kind_label(kind))).clicked() {
                app.settings.agent.providers.push(Provider::new(kind, &kind_label(kind)));
                app.settings_changed();
            }
        }
    });
    ui.add_space(8.0);

    let mut changed = false;
    let mut remove = None;
    let mut check = None;
    for (i, p) in app.settings.agent.providers.iter_mut().enumerate() {
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                theme::section(ui, icon::PLUGS_CONNECTED, &kind_label(p.kind));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::TRASH, &t!("agent.remove_provider")).clicked() {
                        remove = Some(i);
                    }
                });
            });
            theme::row(ui, &t!("agent.provider_name"), |ui| {
                changed |= ui.add(egui::TextEdit::singleline(&mut p.name).desired_width(320.0)).changed();
            });
            if p.kind == ProviderKind::OpenAiCompatible {
                theme::row(ui, &t!("agent.base_url"), |ui| {
                    changed |= ui
                        .add(egui::TextEdit::singleline(&mut p.base_url).hint_text("http://localhost:11434/v1").desired_width(420.0))
                        .on_hover_text(t!("agent.base_url_hint"))
                        .changed();
                });
            }
            let key_label = if p.kind == ProviderKind::HuggingFace { t!("agent.hf_token") } else { t!("agent.api_key") };
            theme::row(ui, &key_label, |ui| {
                let shown = app.agent.providers.shown_keys.contains(&p.id);
                let mut edit = egui::TextEdit::singleline(&mut p.api_key).password(!shown).desired_width(420.0);
                if p.kind == ProviderKind::HuggingFace {
                    edit = edit.hint_text(t!("agent.hf_token_optional"));
                }
                let resp = ui.add(edit);
                let resp = if p.kind == ProviderKind::HuggingFace { resp.on_hover_text(t!("agent.hf_token_hint")) } else { resp };
                changed |= resp.changed();
                let (ic, tip) = if shown { (icon::EYE_SLASH, t!("agent.hide_key")) } else { (icon::EYE, t!("agent.show_key")) };
                if theme::icon_button(ui, ic, &tip).clicked() {
                    if shown {
                        app.agent.providers.shown_keys.retain(|k| *k != p.id);
                    } else {
                        app.agent.providers.shown_keys.push(p.id.clone());
                    }
                }
            });
            theme::row(ui, &t!("agent.proxy"), |ui| {
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut p.proxy).hint_text("socks5://127.0.0.1:1080").desired_width(320.0))
                    .on_hover_text(t!("agent.proxy_hint"))
                    .changed();
            });
            theme::row(ui, &t!("agent.context"), |ui| {
                changed |= ui
                    .add(egui::DragValue::new(&mut p.context).range(2048..=2_000_000).speed(1024).suffix(t!("agent.tokens")))
                    .on_hover_text(t!("agent.provider_context_hint"))
                    .changed();
            });
            theme::row(ui, "", |ui| {
                let running = matches!(app.agent.providers.checks.get(&p.id), Some(CheckState::Running(_)));
                if ui.add_enabled(!running, theme::secondary(icon::CHECK_CIRCLE, &t!("agent.check"))).clicked() {
                    check = Some(p.clone());
                }
                match app.agent.providers.checks.get(&p.id) {
                    Some(CheckState::Running(_)) => {
                        ui.spinner();
                    }
                    Some(CheckState::Done(Ok(models))) => {
                        let pal = theme::palette(ui);
                        ui.label(RichText::new(format!("{}  {}", icon::CHECK, t!("agent.provider_ok", n = models.len()))).color(pal.success))
                            .on_hover_text(models.iter().take(40).cloned().collect::<Vec<_>>().join("\n"));
                    }
                    Some(CheckState::Done(Err(e))) => {
                        ui.colored_label(theme::palette(ui).danger, format!("{}  {e}", icon::WARNING));
                    }
                    None => {}
                }
            });
        });
        ui.add_space(6.0);
    }
    if let Some(p) = check {
        app.agent.providers.start_check(&p);
    }
    if let Some(i) = remove {
        let p = app.settings.agent.providers.remove(i);
        app.agent.providers.checks.remove(&p.id);
        changed = true;
    }
    changed |= web_card(app, ui);
    if changed {
        app.settings_changed();
    }
}

fn web_card(app: &mut App, ui: &mut egui::Ui) -> bool {
    let mut changed = false;
    let mut check = false;
    let web = &mut app.settings.agent.web;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::GLOBE, &t!("agent.web_title"));
        theme::row(ui, "", |ui| {
            changed |= ui.checkbox(&mut web.enabled, t!("agent.web_enabled")).on_hover_text(t!("agent.web_enabled_hint")).changed();
        });
        if !web.enabled {
            return;
        }
        theme::row(ui, &t!("agent.web_engine"), |ui| {
            let opts = [
                (WebEngine::DuckDuckGo, "DuckDuckGo".to_string()),
                (WebEngine::SearXng, "SearXNG".to_string()),
                (WebEngine::Brave, "Brave".to_string()),
                (WebEngine::Tavily, "Tavily".to_string()),
            ];
            changed |= theme::segmented(ui, &mut web.engine, &opts);
        });
        let hint = match web.engine {
            WebEngine::DuckDuckGo => t!("agent.web_ddg_hint"),
            WebEngine::SearXng => t!("agent.web_searxng_hint"),
            WebEngine::Brave | WebEngine::Tavily => t!("agent.web_key_hint"),
        };
        theme::row(ui, "", |ui| theme::hint(ui, hint));
        match web.engine {
            WebEngine::SearXng => theme::row(ui, &t!("agent.base_url"), |ui| {
                changed |= ui.add(egui::TextEdit::singleline(&mut web.searxng_url).hint_text("http://localhost:8888").desired_width(420.0)).changed();
            }),
            WebEngine::Brave | WebEngine::Tavily => theme::row(ui, &t!("agent.api_key"), |ui| {
                changed |= ui.add(egui::TextEdit::singleline(&mut web.api_key).password(true).desired_width(420.0)).changed();
            }),
            WebEngine::DuckDuckGo => {}
        }
        theme::row(ui, &t!("agent.proxy"), |ui| {
            changed |= ui
                .add(egui::TextEdit::singleline(&mut web.proxy).hint_text("socks5://127.0.0.1:1080").desired_width(320.0))
                .on_hover_text(t!("agent.web_proxy_hint"))
                .changed();
        });
        theme::row(ui, "", |ui| {
            let running = matches!(app.agent.providers.web_check, Some(CheckState::Running(_)));
            if ui.add_enabled(!running, theme::secondary(icon::MAGNIFYING_GLASS, &t!("agent.web_check"))).clicked() {
                check = true;
            }
            match &app.agent.providers.web_check {
                Some(CheckState::Running(_)) => {
                    ui.spinner();
                }
                Some(CheckState::Done(Ok(titles))) if titles.is_empty() => {
                    ui.colored_label(theme::palette(ui).danger, format!("{}  {}", icon::WARNING, t!("agent.web_nothing")));
                }
                Some(CheckState::Done(Ok(titles))) => {
                    let pal = theme::palette(ui);
                    ui.label(RichText::new(format!("{}  {}", icon::CHECK, t!("agent.web_ok", n = titles.len()))).color(pal.success)).on_hover_text(titles.join("\n"));
                }
                Some(CheckState::Done(Err(e))) => {
                    ui.colored_label(theme::palette(ui).danger, format!("{}  {e}", icon::WARNING));
                }
                None => {}
            }
        });
    });
    if check {
        let web = app.settings.agent.web.clone();
        app.agent.providers.start_web_check(&web);
    }
    if changed {
        app.agent.providers.web_check = None;
    }
    changed
}
