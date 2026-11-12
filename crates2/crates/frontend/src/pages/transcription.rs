use super::transcription_settings;
use super::Ctx;
use crate::markdown;
use crate::mentions::Roster;
use crate::ui::theme;
use crate::ui_state::TranscriptionTab;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa2_core::formats::MediaKind;
use mediaa2_i18n::t;
use mediaa2_protocol::{ClientCommand, TranscriptionCommand};

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let state = &ctx.view.state.transcription;
    let roster = {
        let mut names = std::collections::BTreeMap::new();
        if let Some(t) = &ctx.ui.transcript {
            for spk in &t.speakers {
                names.insert(spk.index, spk.name.clone());
            }
        }
        Roster::recording(&names, [])
    };

    ui.heading(t!("transcription.title"));
    ui.add_space(8.0);

    ui.horizontal(|ui| {
        for (tab, label) in [
            (
                TranscriptionTab::Assistant,
                t!("transcription.tab_assistant"),
            ),
            (
                TranscriptionTab::Providers,
                t!("transcription.tab_providers"),
            ),
            (TranscriptionTab::Models, t!("transcription.tab_models")),
            (TranscriptionTab::Budget, t!("transcription.tab_budget")),
        ] {
            if ui
                .selectable_label(ctx.ui.transcription_tab == tab, label)
                .clicked()
            {
                ctx.ui.transcription_tab = tab;
            }
        }
    });
    ui.add_space(4.0);
    match ctx.ui.transcription_tab {
        TranscriptionTab::Providers => {
            transcription_settings::providers(ctx, ui);
            return;
        }
        TranscriptionTab::Models => {
            transcription_settings::models(ctx, ui);
            return;
        }
        TranscriptionTab::Budget => {
            transcription_settings::budget(ctx, ui);
            return;
        }
        TranscriptionTab::Assistant => {}
    }

    if state.running {
        if let Some(task) = &state.task {
            ui.horizontal(|ui| {
                ui.label(t!("transcription.running_task"));
                ui.strong(task);
                if let (Some(progress), Some(status)) = (state.progress, &state.status) {
                    ui.label(status.to_string());
                    ui.add(egui::ProgressBar::new(progress as f32 / 100.0).show_percentage());
                }
            });
            ui.add_space(4.0);
        }
    }

    if let Some(snapshot) = &state.snapshot {
        if !snapshot.tasks.is_empty() {
            ui.label(t!("transcription.queue"));
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(200.0)
                .show(ui, |ui| {
                    for task in &snapshot.tasks {
                        ui.horizontal(|ui| {
                            let color = match task.status {
                            mediaa2_protocol::transcription::TranscriptionTaskStatus::Running => {
                                egui::Color32::from_rgb(80, 200, 80)
                            }
                            mediaa2_protocol::transcription::TranscriptionTaskStatus::Failed => {
                                egui::Color32::from_rgb(220, 60, 60)
                            }
                            mediaa2_protocol::transcription::TranscriptionTaskStatus::Done => {
                                egui::Color32::from_rgb(100, 180, 255)
                            }
                            _ => egui::Color32::GRAY,
                        };
                            ui.colored_label(color, format!("{}: {}", task.kind, task.title));
                            if task.progress > 0.0 && task.progress < 1.0 {
                                ui.add(
                                    egui::ProgressBar::new(task.progress)
                                        .desired_width(120.0)
                                        .show_percentage(),
                                );
                            }
                            if let Some(err) = &task.error {
                                ui.colored_label(egui::Color32::RED, err);
                            }
                            let active = matches!(
                            task.status,
                            mediaa2_protocol::transcription::TranscriptionTaskStatus::Queued
                                | mediaa2_protocol::transcription::TranscriptionTaskStatus::Running
                        );
                            if active
                                && ui
                                    .small_button(icon::X)
                                    .on_hover_text(t!("transcription.cancel"))
                                    .clicked()
                            {
                                ctx.actions.send(ClientCommand::Transcription(
                                    TranscriptionCommand::CancelTask { task: task.id },
                                ));
                            }
                        });
                    }
                });
            ui.add_space(8.0);
        }
    }

    if let Some(streaming) = &state.streaming_text {
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(icon::PAPER_PLANE_RIGHT.to_string());
            ui.strong(t!("transcription.assistant_typing"));
        });
        egui::ScrollArea::vertical()
            .max_height(200.0)
            .show(ui, |ui| {
                markdown::show(
                    ui,
                    &markdown::linkify(streaming, true),
                    &roster,
                    theme::palette(ui),
                );
            });
        ui.add_space(4.0);
    }

    if let Some(timeline) = &ctx.ui.chat_timeline {
        ui.separator();
        ui.heading(t!("transcription.chat_title"));
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() * 0.4)
            .show(ui, |ui| {
                for entry in &timeline.entries {
                    match entry {
                        mediaa2_core::chat::TimelineEntry::Answer { text, .. } => {
                            ui.horizontal(|ui| {
                                ui.colored_label(
                                    egui::Color32::from_rgb(100, 180, 255),
                                    icon::ROBOT.to_string(),
                                );
                            });
                            markdown::show(
                                ui,
                                &markdown::linkify(text, true),
                                &roster,
                                theme::palette(ui),
                            );
                        }
                        mediaa2_core::chat::TimelineEntry::Thinking { text } => {
                            ui.horizontal(|ui| {
                                ui.colored_label(egui::Color32::GRAY, icon::CLOUD.to_string());
                                ui.label(text);
                            });
                        }
                        mediaa2_core::chat::TimelineEntry::Step { title, .. } => {
                            ui.horizontal(|ui| {
                                ui.colored_label(egui::Color32::GRAY, icon::GEAR.to_string());
                                ui.label(title);
                            });
                        }
                        _ => {}
                    }
                }
            });
        ui.add_space(4.0);
    }

    if let Some(error) = &ctx.view.state.transcription.chat_error {
        ui.horizontal(|ui| {
            ui.colored_label(
                egui::Color32::from_rgb(220, 60, 60),
                icon::WARNING.to_string(),
            );
            ui.label(chat_error_message(&error.kind));
        });
        ui.label(
            egui::RichText::new(&error.detail)
                .small()
                .color(egui::Color32::GRAY),
        );
        ui.add_space(4.0);
    }

    let topics = ctx.ui.topics.clone();
    if let Some(topics) = &topics {
        show_topics(ctx, ui, topics);
        return;
    }

    let transcript = ctx.ui.transcript.clone();
    if let Some(transcript) = &transcript {
        show_transcript(ctx, ui, transcript);
        return;
    }

    ui.separator();
    ui.heading(t!("transcription.start"));
    ui.add_space(4.0);

    let media_entries: Vec<_> = ctx
        .view
        .library
        .entries
        .iter()
        .filter(|e| matches!(e.kind, MediaKind::Video | MediaKind::Audio))
        .collect();

    if media_entries.is_empty() {
        ui.label(t!("transcription.no_media"));
    } else {
        egui::ScrollArea::vertical().show(ui, |ui| {
            for entry in &media_entries {
                ui.horizontal(|ui| {
                    let kind_icon = match entry.kind {
                        MediaKind::Video => icon::VIDEO_CAMERA,
                        _ => icon::MICROPHONE,
                    };
                    ui.label(kind_icon.to_string());
                    ui.label(&entry.title);
                    if ui
                        .small_button(icon::TEXT_T)
                        .on_hover_text(t!("transcription.start"))
                        .clicked()
                    {
                        ctx.actions.send(ClientCommand::Transcription(
                            TranscriptionCommand::Start {
                                path: entry.path.to_string_lossy().into_owned(),
                                options: Default::default(),
                                from: None,
                            },
                        ));
                    }
                    if ui
                        .small_button(icon::EYE)
                        .on_hover_text(t!("transcription.view_transcript"))
                        .clicked()
                    {
                        ctx.actions.send(ClientCommand::Transcription(
                            TranscriptionCommand::LoadTranscript {
                                path: entry.path.to_string_lossy().into_owned(),
                            },
                        ));
                    }
                    if ui
                        .small_button(icon::LIST_BULLETS)
                        .on_hover_text(t!("transcription.view_topics"))
                        .clicked()
                    {
                        ctx.actions.send(ClientCommand::Transcription(
                            TranscriptionCommand::LoadTopics {
                                path: entry.path.to_string_lossy().into_owned(),
                            },
                        ));
                    }
                });
            }
        });
    }

    ui.separator();
    ui.heading(t!("transcription.chat_title"));
    ui.add_space(4.0);
    chat_composer(ctx, ui, &media_entries);
}

fn chat_error_message(kind: &str) -> String {
    match kind {
        "unauthorized" => t!("transcription.error_unauthorized").into(),
        "rate_limited" => t!("transcription.error_rate_limited").into(),
        "network" => t!("transcription.error_network").into(),
        "not_found" => t!("transcription.error_not_found").into(),
        _ => t!("transcription.error_other").into(),
    }
}

fn chat_composer(
    ctx: &mut Ctx<'_>,
    ui: &mut egui::Ui,
    media_entries: &[&mediaa2_core::library::LibraryEntry],
) {
    let mut question = String::new();
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut question)
                .hint_text(t!("transcription.chat_hint"))
                .desired_width(ui.available_width() - 60.0),
        );
        let sent = (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            || ui.button(icon::PAPER_PLANE_RIGHT.to_string()).clicked();
        let entry = media_entries.first().copied();
        let entry = entry.filter(|_| sent && !question.trim().is_empty());
        if let Some(entry) = entry {
            let question = question.trim().to_string();
            let store = entry.path.to_string_lossy().into_owned();
            ctx.ui.chat_store = Some(store.clone());
            // an existing chat keeps its model: a changed choice needs consent
            let current = ctx
                .view
                .state
                .settings
                .agent
                .models
                .agent
                .as_ref()
                .map(|s| s.model.clone());
            let pinned = ctx.ui.chat_model.as_ref().map(|m| m.model.clone());
            if ctx.ui.chat_id.is_some() && pinned.is_some() && pinned != current {
                ctx.ui.chat_confirm = Some(question);
                return;
            }
            send_question(ctx, entry, &store, &question, false);
        }
    });
    chat_confirm_modal(ctx, ui);
}

fn send_question(
    ctx: &mut Ctx<'_>,
    entry: &mediaa2_core::library::LibraryEntry,
    store: &str,
    question: &str,
    switch_model: bool,
) {
    let locale = ctx
        .view
        .state
        .settings
        .language
        .clone()
        .unwrap_or_else(|| "en".into());
    ctx.actions
        .send(ClientCommand::Transcription(TranscriptionCommand::Send {
            store: store.to_string(),
            chat: ctx.ui.chat_id,
            question: question.to_string(),
            locale,
            path: Some(entry.path.to_string_lossy().into_owned()),
            files: vec![],
            duration: None,
            position: None,
            stored: false,
            switch_model,
        }));
}

fn chat_confirm_modal(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let Some(question) = ctx.ui.chat_confirm.clone() else {
        return;
    };
    let (Some(pinned), Some(current)) = (
        ctx.ui.chat_model.clone(),
        ctx.view
            .state
            .settings
            .agent
            .models
            .agent
            .as_ref()
            .map(|s| s.model.clone()),
    ) else {
        ctx.ui.chat_confirm = None;
        return;
    };
    let entry = ctx.ui.chat_store.as_ref().and_then(|store| {
        ctx.view
            .library
            .entries
            .iter()
            .find(|e| e.path.to_string_lossy() == *store)
            .cloned()
    });
    let Some(entry) = entry else {
        ctx.ui.chat_confirm = None;
        return;
    };
    let store = ctx.ui.chat_store.clone().unwrap_or_default();
    egui::Modal::new(egui::Id::new("chat-model-confirm")).show(ui.ctx(), |ui| {
        ui.heading(t!("transcription.confirm_title"));
        ui.add_space(4.0);
        ui.label(t!(
            "transcription.confirm_text",
            pinned = pinned.model,
            current = current
        ));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .button(t!("transcription.confirm_keep", pinned = pinned.model))
                .clicked()
            {
                ctx.ui.chat_confirm = None;
                send_question(ctx, &entry, &store, &question, false);
            }
            if ui
                .button(t!("transcription.confirm_switch", current = current))
                .clicked()
            {
                ctx.ui.chat_confirm = None;
                send_question(ctx, &entry, &store, &question, true);
            }
            if ui.button(icon::X.to_string()).clicked() {
                ctx.ui.chat_confirm = None;
            }
        });
    });
}

fn show_topics(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, topics: &[mediaa2_protocol::TopicChapter]) {
    ui.heading(t!("transcription.topics_title"));
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .max_height(ui.available_height() - 60.0)
        .show(ui, |ui| {
            for (idx, chapter) in topics.iter().enumerate() {
                let header = format!(
                    "{}. {}  [{:.0}—{:.0}s]",
                    idx + 1,
                    chapter.title,
                    chapter.start,
                    chapter.end
                );
                egui::CollapsingHeader::new(header)
                    .default_open(idx == 0)
                    .show(ui, |ui| {
                        if !chapter.summary.is_empty() {
                            ui.label(&chapter.summary);
                            ui.add_space(4.0);
                        }
                        for section in &chapter.sections {
                            ui.strong(&section.title);
                            for item in &section.items {
                                ui.horizontal(|ui| {
                                    if let Some(time) = item.time {
                                        ui.monospace(format!("[{:.0}s]", time));
                                    }
                                    ui.label(&item.text);
                                });
                            }
                            ui.add_space(2.0);
                        }
                        if !chapter.decisions.is_empty() {
                            ui.strong(t!("transcription.decisions"));
                            for d in &chapter.decisions {
                                ui.label(format!("  • {d}"));
                            }
                            ui.add_space(2.0);
                        }
                        if !chapter.actions.is_empty() {
                            ui.strong(t!("transcription.actions"));
                            for a in &chapter.actions {
                                ui.label(format!("  • {a}"));
                            }
                        }
                    });
            }
        });

    ui.add_space(8.0);
    if ui.button(t!("transcription.close_topics")).clicked() {
        ctx.ui.topics = None;
        ctx.ui.topics_path = None;
    }
}

fn show_transcript(
    ctx: &mut Ctx<'_>,
    ui: &mut egui::Ui,
    transcript: &mediaa2_protocol::TranscriptView,
) {
    ui.heading(t!("transcription.transcript_title"));
    ui.add_space(4.0);

    // the embedding index of the current model, with a manual build
    ui.horizontal(|ui| {
        let status = if transcript.index_building {
            t!("transcription.index_building").into_owned()
        } else if transcript.index_ready {
            t!("transcription.index_ready").into_owned()
        } else {
            t!("transcription.index_missing").into_owned()
        };
        ui.label(format!("{}: {}", t!("transcription.index_label"), status));
        if !transcript.index_ready
            && !transcript.index_building
            && ui.button(t!("transcription.index_build")).clicked()
        {
            ctx.actions
                .send(ClientCommand::Transcription(TranscriptionCommand::Start {
                    path: transcript.path.clone(),
                    options: mediaa2_core::agent::TranscriptionOptions {
                        index: true,
                        ..Default::default()
                    },
                    from: Some("index".into()),
                }));
        }
    });
    ui.add_space(4.0);

    egui::ScrollArea::vertical()
        .max_height(ui.available_height() - 60.0)
        .show(ui, |ui| {
            for reply in &transcript.replies {
                ui.horizontal(|ui| {
                    let spk = reply.speaker.unwrap_or(0);
                    let color = speaker_color(spk);
                    let (chip, _) =
                        ui.allocate_exact_size(egui::vec2(50.0, 18.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(chip, egui::CornerRadius::same(4), color);
                    ui.painter().text(
                        chip.center(),
                        egui::Align2::CENTER_CENTER,
                        format!("@s{spk}"),
                        egui::FontId::proportional(11.0),
                        egui::Color32::WHITE,
                    );
                    ui.label(format!("[{:.0}s]", reply.start));
                    ui.label(&reply.text);
                });
            }
        });

    ui.separator();
    ui.separator();
    ui.heading(t!("transcription.speakers"));
    for speaker in &transcript.speakers {
        ui.horizontal(|ui| {
            let color = speaker_color(speaker.index);
            let (chip, _) = ui.allocate_exact_size(egui::vec2(50.0, 18.0), egui::Sense::hover());
            ui.painter()
                .rect_filled(chip, egui::CornerRadius::same(4), color);
            ui.painter().text(
                chip.center(),
                egui::Align2::CENTER_CENTER,
                format!("@s{}", speaker.index),
                egui::FontId::proportional(11.0),
                egui::Color32::WHITE,
            );
            if ctx.ui.renaming_speaker == Some(speaker.index) {
                let response = ui
                    .add(egui::TextEdit::singleline(&mut ctx.ui.rename_value).desired_width(150.0));
                if (response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    || ui.small_button(icon::CHECK).clicked()
                {
                    let name = ctx.ui.rename_value.clone();
                    if let Some(store) = &ctx.ui.chat_store {
                        ctx.actions.send(ClientCommand::Transcription(
                            TranscriptionCommand::RenameSpeaker {
                                store: store.clone(),
                                speaker: speaker.index,
                                name,
                            },
                        ));
                    }
                    ctx.ui.renaming_speaker = None;
                    ctx.ui.rename_value = String::new();
                }
                if ui.small_button(icon::X).clicked() {
                    ctx.ui.renaming_speaker = None;
                }
            } else {
                ui.label(&speaker.name);
                if ui
                    .small_button(icon::PENCIL_SIMPLE)
                    .on_hover_text("Rename")
                    .clicked()
                {
                    ctx.ui.renaming_speaker = Some(speaker.index);
                    ctx.ui.rename_value = speaker.name.clone();
                    ctx.ui.chat_store = Some(transcript.file_id.clone());
                }
                if let Some(suggested) = &speaker.suggested {
                    // the voice match is close but not sure enough to apply
                    ui.label(
                        egui::RichText::new(format!(
                            "{} {}",
                            t!("transcription.sounds_like"),
                            suggested
                        ))
                        .weak(),
                    );
                    if ui
                        .small_button(icon::CHECK)
                        .on_hover_text(t!("transcription.accept_suggestion"))
                        .clicked()
                    {
                        ctx.actions.send(ClientCommand::Transcription(
                            TranscriptionCommand::RenameSpeaker {
                                store: transcript.file_id.clone(),
                                speaker: speaker.index,
                                name: suggested.clone(),
                            },
                        ));
                    }
                    if ui
                        .small_button(icon::X)
                        .on_hover_text(t!("transcription.dismiss_suggestion"))
                        .clicked()
                    {
                        ctx.actions.send(ClientCommand::Transcription(
                            TranscriptionCommand::DismissSuggestion {
                                store: transcript.file_id.clone(),
                                speaker: speaker.index,
                            },
                        ));
                    }
                }
            }
        });
    }
    ui.add_space(8.0);
    if ui.button(t!("transcription.close_transcript")).clicked() {
        ctx.ui.transcript = None;
        ctx.ui.renaming_speaker = None;
    }
}

fn speaker_color(index: u32) -> egui::Color32 {
    const COLORS: [egui::Color32; 8] = [
        egui::Color32::from_rgb(0x4E, 0x79, 0xA7),
        egui::Color32::from_rgb(0xF2, 0x8E, 0x2B),
        egui::Color32::from_rgb(0xE1, 0x57, 0x59),
        egui::Color32::from_rgb(0x76, 0xB7, 0xB2),
        egui::Color32::from_rgb(0x59, 0xA1, 0x4F),
        egui::Color32::from_rgb(0xED, 0xC9, 0x48),
        egui::Color32::from_rgb(0xB0, 0x7A, 0xA1),
        egui::Color32::from_rgb(0xFF, 0x9D, 0xA7),
    ];
    COLORS[(index as usize) % COLORS.len()]
}
