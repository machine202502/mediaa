use crate::icon::{badge, Badge};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use rust_i18n::t;
use tray_icon::menu::{IconMenuItem, Menu, MenuEvent, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCommand {
    PrimaryClick,
    ShowWindow,
    RegionScreenshot,
    FullscreenScreenshot,
    StartRecording,
    StartAudio,
    PauseResume,
    StopAndSave,
    CancelRecording,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Starting,
    Recording,
    Paused,
    Saving,
}

const ITEMS: [(&str, TrayCommand); 9] = [
    ("show", TrayCommand::ShowWindow),
    ("region", TrayCommand::RegionScreenshot),
    ("fullscreen", TrayCommand::FullscreenScreenshot),
    ("start", TrayCommand::StartRecording),
    ("audio", TrayCommand::StartAudio),
    ("pause", TrayCommand::PauseResume),
    ("stop", TrayCommand::StopAndSave),
    ("cancel", TrayCommand::CancelRecording),
    ("quit", TrayCommand::Quit),
];

fn command_for(id: &str) -> Option<TrayCommand> {
    ITEMS.iter().find(|(i, _)| *i == id).map(|(_, c)| *c)
}

fn tray_icon_rgba(state: TrayState, progress: bool) -> Vec<u8> {
    if !progress {
        return crate::icon::rgba(32);
    }
    match state {
        TrayState::Idle => crate::icon::rgba(32),
        TrayState::Starting => crate::icon::rgba_status(32, crate::icon::StatusDot::Saving),
        TrayState::Recording => crate::icon::rgba_status(32, crate::icon::StatusDot::Recording),
        TrayState::Paused => crate::icon::rgba_status(32, crate::icon::StatusDot::Paused),
        TrayState::Saving => crate::icon::rgba_status(32, crate::icon::StatusDot::Saving),
    }
}

pub struct Tray {
    icon: TrayIcon,
    items: Vec<(TrayCommand, IconMenuItem)>,
    applied: Option<(TrayState, bool, String)>,
}

impl Tray {
    pub fn new(ctx: &egui::Context) -> anyhow::Result<(Tray, Receiver<TrayCommand>)> {
        let items: Vec<(TrayCommand, IconMenuItem)> = ITEMS
            .iter()
            .map(|(id, cmd)| (*cmd, IconMenuItem::with_id(*id, *id, true, None, None)))
            .collect();
        let get = |c: TrayCommand| {
            &items
                .iter()
                .find(|(x, _)| *x == c)
                .expect("all items exist")
                .1
        };
        let menu = Menu::new();
        menu.append_items(&[
            get(TrayCommand::ShowWindow),
            &PredefinedMenuItem::separator(),
            get(TrayCommand::RegionScreenshot),
            get(TrayCommand::FullscreenScreenshot),
            &PredefinedMenuItem::separator(),
            get(TrayCommand::StartRecording),
            get(TrayCommand::StartAudio),
            get(TrayCommand::PauseResume),
            get(TrayCommand::StopAndSave),
            get(TrayCommand::CancelRecording),
            &PredefinedMenuItem::separator(),
            get(TrayCommand::Quit),
        ])?;
        let icon = Icon::from_rgba(crate::icon::rgba(32), 32, 32)?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("MediaA")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()?;

        let (tx, rx) = crossbeam_channel::bounded(32);
        install_handlers(tx, ctx.clone());
        let mut tray = Tray {
            icon: tray,
            items,
            applied: None,
        };
        tray.set_state(TrayState::Idle, true);
        Ok((tray, rx))
    }

    fn item(&self, c: TrayCommand) -> &IconMenuItem {
        &self
            .items
            .iter()
            .find(|(x, _)| *x == c)
            .expect("all items exist")
            .1
    }

    pub fn set_state(&mut self, state: TrayState, progress: bool) {
        let locale = crate::i18n::current();
        if self.applied.as_ref() == Some(&(state, progress, locale.clone())) {
            return;
        }
        self.applied = Some((state, progress, locale));
        let active = matches!(state, TrayState::Recording | TrayState::Paused);
        let cancel_available = active || state == TrayState::Starting;
        for (cmd, text, enabled) in [
            (TrayCommand::ShowWindow, t!("tray.show"), true),
            (TrayCommand::RegionScreenshot, t!("tray.region"), true),
            (
                TrayCommand::FullscreenScreenshot,
                t!("tray.fullscreen"),
                true,
            ),
            (
                TrayCommand::StartRecording,
                t!("tray.start"),
                state == TrayState::Idle,
            ),
            (
                TrayCommand::StartAudio,
                t!("tray.audio"),
                state == TrayState::Idle,
            ),
            (
                TrayCommand::PauseResume,
                if state == TrayState::Paused {
                    t!("tray.resume")
                } else {
                    t!("tray.pause")
                },
                active,
            ),
            (TrayCommand::StopAndSave, t!("tray.stop"), active),
            (
                TrayCommand::CancelRecording,
                t!("tray.cancel"),
                cancel_available,
            ),
            (TrayCommand::Quit, t!("tray.quit"), true),
        ] {
            let item = self.item(cmd);
            item.set_text(text);
            item.set_enabled(enabled);
        }
        let mark = |b: Badge| tray_icon::menu::Icon::from_rgba(badge(b, 32), 32, 32).ok();
        let idle = state == TrayState::Idle;
        self.item(TrayCommand::RegionScreenshot).set_icon(if idle {
            mark(Badge::Area)
        } else {
            None
        });
        self.item(TrayCommand::StartRecording).set_icon(if idle {
            mark(Badge::Record)
        } else {
            None
        });
        self.item(TrayCommand::StopAndSave)
            .set_icon(if active { mark(Badge::Stop) } else { None });
        let tip = if !progress {
            "MediaA".to_string()
        } else {
            match state {
                TrayState::Idle => "MediaA".to_string(),
                TrayState::Starting => format!("MediaA — {}", t!("record.starting")),
                TrayState::Recording => format!("MediaA — {}", t!("record.recording")),
                TrayState::Paused => format!("MediaA — {}", t!("record.paused")),
                TrayState::Saving => format!("MediaA — {}", t!("record.saving")),
            }
        };
        let _ = self.icon.set_tooltip(Some(tip));
        let rgba = tray_icon_rgba(state, progress);
        if let Ok(icon) = Icon::from_rgba(rgba, 32, 32) {
            let _ = self.icon.set_icon(Some(icon));
        }
    }
}

fn install_handlers(tx: Sender<TrayCommand>, ctx: egui::Context) {
    let (menu_tx, menu_ctx) = (tx.clone(), ctx.clone());
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if let Some(cmd) = command_for(event.id.as_ref()) {
            let _ = menu_tx.try_send(cmd);
            menu_ctx.request_repaint();
        }
    }));
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        let left_click = matches!(
            event,
            TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } | TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            }
        );
        if left_click {
            let _ = tx.try_send(TrayCommand::PrimaryClick);
            ctx.request_repaint();
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_menu_id_maps_to_a_command() {
        for (id, cmd) in ITEMS {
            assert_eq!(command_for(id), Some(cmd));
        }
        assert_eq!(command_for("nope"), None);
    }

    #[test]
    fn the_progress_switch_removes_and_restores_the_status_dot() {
        let base = crate::icon::rgba(32);
        assert_eq!(tray_icon_rgba(TrayState::Recording, false), base);
        assert_eq!(tray_icon_rgba(TrayState::Saving, false), base);
        assert_ne!(tray_icon_rgba(TrayState::Recording, true), base);
        assert_ne!(tray_icon_rgba(TrayState::Saving, true), base);
    }
}
