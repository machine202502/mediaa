use crate::ipc::{self, Shutdown, UiMailbox};
use crate::services::presence::{PresenceDeps, PresenceHandle, PresenceOptions};
use crate::services::{
    ConversionHandle, LibraryHandle, NoticeRing, PreviewHandle, RecordingHandle, ScreenshotHandle,
    SettingsHandle,
};
use crate::state::StateStore;
use mediaa2_core::paths;
use mediaa2_core::settings::Settings;
use mediaa2_platform::single_instance::SingleInstance;
use mediaa2_protocol::state::{ConversionState, RecordingState, RecoveryState, State};
use std::sync::Arc;

pub fn run() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async_run())
}

async fn async_run() -> anyhow::Result<()> {
    mediaa2_core::logging::init_named(
        &paths::logs_dir(),
        "mediaa2-backend.log",
        log::LevelFilter::Info,
    );
    log::info!("MediaA 2 backend starting");
    let Some(_instance) = SingleInstance::acquire()? else {
        return Ok(());
    };
    let settings = Settings::load();
    mediaa2_i18n::set_language(settings.language.as_deref(), None);
    mediaa2_media::preview_cache::set_animated_previews_enabled(settings.animated_previews);
    mediaa2_media::preview_cache::set_full_previews_enabled(settings.full_preview_videos);
    let state = Arc::new(StateStore::new(State {
        settings: settings.clone(),
        recording: RecordingState::Idle,
        conversion: ConversionState::Idle,
        recovery: RecoveryState { running: false },
        notices: Vec::new(),
        picker: None,
        library_revision: 0,
    }));
    let library = LibraryHandle::open(settings.media_dir.clone())?;
    if let Err(error) = library.refresh() {
        log::warn!("library: initial scan failed: {error:#}");
    }
    state.update(|current| current.library_revision = library.revision());
    let notices = NoticeRing::new();
    let recording = RecordingHandle::new(Arc::clone(&state), library.clone(), notices.clone());
    if let Err(error) = recording.recover().await {
        log::warn!("recording recovery: {}", error.message);
    }
    let conversion = ConversionHandle::new(Arc::clone(&state), library.clone(), notices.clone());
    let screenshots = ScreenshotHandle::new(Arc::clone(&state), library.clone(), notices.clone());
    let previews = PreviewHandle::new();
    let settings = SettingsHandle::new(Arc::clone(&state), paths::settings_file());
    let shutdown = Shutdown::new();
    let (mailbox, signals) = UiMailbox::new();
    let presence = PresenceHandle::new(PresenceDeps {
        state: Arc::clone(&state),
        mailbox: mailbox.clone(),
        signals,
        screenshots: screenshots.clone(),
        recording: recording.clone(),
        conversion: conversion.clone(),
        notices: notices.clone(),
        shutdown: shutdown.clone(),
        options: PresenceOptions::default(),
    });
    ipc::serve(
        state,
        settings,
        library,
        recording,
        conversion,
        screenshots,
        previews,
        presence,
        mailbox,
        shutdown,
    )
    .await
}
