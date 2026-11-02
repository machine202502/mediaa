use super::*;

impl Daemon {
    pub(super) fn handle_accept(&mut self, accepted: Accepted) {
        match accepted {
            Accepted::Launcher(mut connection) => {
                let mut metadata = std::collections::BTreeMap::new();
                metadata.insert("tray".to_string(), self.tray.is_some().to_string());
                metadata.insert("ui".to_string(), self.ui.is_some().to_string());
                if let Err(error) = connection.send_frame(&Frame::Welcome {
                    version: mediaa_sync::PROTOCOL_VERSION,
                    session: 0,
                    event_seq: 0,
                    metadata,
                }) {
                    log::warn!("ipc welcome: {error}");
                    return;
                }
                let reader = match connection.try_clone() {
                    Ok(reader) => reader,
                    Err(error) => {
                        log::warn!("ipc clone: {error}");
                        return;
                    }
                };
                let tx = self.ui_tx.clone();
                std::thread::spawn(move || {
                    let mut reader = reader;
                    while let Ok(frame) = reader.recv_frame() {
                        if tx.send(UiEvent::Launcher(frame)).is_err() {
                            return;
                        }
                    }
                });
            }
            Accepted::Capture(mut connection) => {
                if let Err(error) = connection.send_frame(&Frame::Welcome {
                    version: mediaa_sync::PROTOCOL_VERSION,
                    session: 0,
                    event_seq: 0,
                    metadata: std::collections::BTreeMap::new(),
                }) {
                    log::warn!("capture welcome: {error}");
                    return;
                }
                let reader = match connection.try_clone() {
                    Ok(reader) => reader,
                    Err(error) => {
                        log::warn!("capture clone: {error}");
                        return;
                    }
                };
                let tx = self.ui_tx.clone();
                std::thread::spawn(move || {
                    let mut reader = reader;
                    while let Ok(frame) = reader.recv_frame() {
                        let writer = match connection.try_clone() {
                            Ok(writer) => writer,
                            Err(_) => return,
                        };
                        if tx.send(UiEvent::Capture(writer, frame)).is_err() {
                            return;
                        }
                    }
                });
            }
            Accepted::Ui(mut connection) => {
                if self.agent_host.is_some() {
                    log::info!("agent host stays in the backend while the ui is open");
                }
                if self.ui.is_some() {
                    let _ = connection.send_frame(&Frame::Reject {
                        reason: ipc::UI_ALREADY_OPEN.into(),
                    });
                    return;
                }
                self.sync = Server::new();
                let hello = Frame::Hello {
                    version: mediaa_sync::PROTOCOL_VERSION,
                    client: ipc::CLIENT_UI.into(),
                };
                match self.sync.on_frame(hello) {
                    ServerAction::Frames(frames) => {
                        for mut frame in frames {
                            if let Frame::Welcome { metadata, .. } = &mut frame {
                                metadata
                                    .insert("tray".to_string(), self.tray.is_some().to_string());
                            }
                            if let Err(error) = connection.send_frame(&frame) {
                                log::warn!("ipc welcome: {error}");
                                return;
                            }
                        }
                    }
                    ServerAction::Command(_) => return,
                }
                let reader = match connection.try_clone() {
                    Ok(reader) => reader,
                    Err(error) => {
                        log::warn!("ipc clone: {error}");
                        return;
                    }
                };
                self.ui = Some(connection);
                self.last_ping = Instant::now();
                self.last_pong = Instant::now();
                self.tray_event(TrayUiEvent::UiConnected, Instant::now());
                let tx = self.ui_tx.clone();
                std::thread::spawn(move || {
                    let mut reader = reader;
                    while let Ok(frame) = reader.recv_frame() {
                        if tx.send(UiEvent::Sync(frame)).is_err() {
                            return;
                        }
                    }
                    let _ = tx.send(UiEvent::Closed);
                });
                self.publish_settings();
                self.send_recording_state();
                self.publish_library(Vec::new(), true);
                self.publish_notices();
                self.publish_recovery();
            }
        }
    }

    pub(super) fn capture_command(
        &mut self,
        name: &str,
        payload: &[u8],
    ) -> Result<Vec<u8>, String> {
        match name {
            mediaa_protocol::backend::CAPTURE_REQUEST => {
                let capture = mediaa_protocol::backend::decode_capture_request(payload)?;
                let source = crate::features::capture::capture_target_file(
                    &self.settings.media_dir,
                    capture.target,
                )?;
                Ok(mediaa_protocol::backend::encode_capture_source(&source))
            }
            mediaa_protocol::backend::SCREENSHOT_COMMIT => {
                let commit = mediaa_protocol::backend::decode_screenshot_commit(payload)?;
                if self.clipboard.is_none() {
                    self.clipboard = Some(crate::features::clipboard::ClipboardService::new(
                        Wake::default(),
                    ));
                }
                let result = match self.clipboard.as_mut() {
                    Some(service) => {
                        let handle = service.handle();
                        crate::features::capture::commit_screenshot(&commit, &handle)
                    }
                    None => Err("clipboard is unavailable".into()),
                }?;
                Ok(mediaa_protocol::backend::encode_shot_result(&result))
            }
            other => Err(format!("unsupported capture command: {other}")),
        }
    }

    pub(super) fn handle_ui_event(&mut self, event: UiEvent) {
        match event {
            UiEvent::Sync(frame) => self.handle_sync_frame(frame),
            UiEvent::Capture(mut connection, frame) => {
                if let Frame::Command { id, name, payload } = frame {
                    let result = self.capture_command(&name, &payload);
                    let (ok, payload, error) = match result {
                        Ok(payload) => (true, payload, None),
                        Err(error) => (false, Vec::new(), Some(error)),
                    };
                    if let Err(error) = connection.send_frame(&Frame::Result {
                        id,
                        ok,
                        payload,
                        error,
                    }) {
                        log::warn!("capture result: {error}");
                    }
                }
            }
            UiEvent::Launcher(frame) => {
                if let Frame::Command { name, .. } = frame {
                    match name.as_str() {
                        mediaa_protocol::PICKER_STARTING if self.ui.is_some() => {
                            self.forward_event(
                                mediaa_protocol::TRAY_EVENT,
                                mediaa_protocol::encode_tray_command(
                                    mediaa_protocol::TrayCommandWire::RegionScreenshot,
                                ),
                            );
                        }
                        mediaa_protocol::PICKER_STARTING => self.show_mask(),
                        mediaa_protocol::SHOW_WINDOW => {
                            log::info!("second launch: showing the existing ui");
                            self.tray_event(TrayUiEvent::Show, Instant::now());
                        }
                        _ => {}
                    }
                }
            }
            UiEvent::Closed => {
                log::info!("the ui disconnected; keeping the daemon in the tray");
                self.ui = None;
                self.tray_event(TrayUiEvent::UiDisconnected, Instant::now());
                self.hide_mask();
            }
        }
    }

    pub(super) fn forward_event(&mut self, kind: &str, payload: Vec<u8>) {
        if self.ui.is_some() {
            self.push_event(kind, payload);
            return;
        }
        self.pending_events.push((kind.to_string(), payload));
        self.ensure_ui();
    }

    pub(super) fn ensure_ui(&mut self) {
        if self.ui.is_some() {
            return;
        }
        if let Err(error) = spawn_ui(&hidden_args()) {
            log::warn!("spawn the ui: {error}");
        }
    }

    pub(super) fn ensure_ui_visible(&mut self) {
        if self.ui.is_some() {
            return;
        }
        if let Err(error) = spawn_ui(&["--show".to_string()]) {
            log::warn!("spawn the visible ui: {error}");
        }
    }

    pub(super) fn push_event(&mut self, kind: &str, payload: Vec<u8>) {
        self.sync.events_mut().push(kind, payload);
        self.flush_sync();
    }

    pub(super) fn flush_pending_events(&mut self) {
        let pending = std::mem::take(&mut self.pending_events);
        for (kind, payload) in pending {
            self.push_event(&kind, payload);
        }
    }

    pub(super) fn flush_sync(&mut self) {
        for frame in self.sync.flush() {
            self.send_frame(frame);
        }
    }

    pub(super) fn send_frame(&mut self, frame: Frame) {
        let Some(ui) = self.ui.as_mut() else {
            return;
        };
        if let Err(error) = ui.send_frame(&frame) {
            log::warn!("ipc sync send: {error}");
            self.ui = None;
        }
    }

    pub(super) fn handle_sync_frame(&mut self, frame: Frame) {
        if matches!(frame, Frame::Pong) {
            self.last_pong = Instant::now();
        }
        let subscribe = matches!(frame, Frame::Subscribe { .. });
        match self.sync.on_frame(frame) {
            ServerAction::Frames(frames) => {
                for frame in frames {
                    self.send_frame(frame);
                }
            }
            ServerAction::Command(request) => {
                let result = match request.name.as_str() {
                    mediaa_protocol::HOTKEYS_PAUSED => {
                        match serde_json::from_slice::<bool>(&request.payload) {
                            Ok(true) => {
                                if let Some(hotkeys) = self.hotkeys.as_mut() {
                                    hotkeys.apply(Vec::new());
                                }
                                Ok(Vec::new())
                            }
                            Ok(false) => {
                                self.apply_hotkeys();
                                Ok(Vec::new())
                            }
                            Err(error) => Err(error.to_string()),
                        }
                    }
                    mediaa_protocol::TRAY_STATE => {
                        match mediaa_protocol::decode_tray_update(&request.payload) {
                            Ok(update) => {
                                if let Some(tray) = self.tray.as_mut() {
                                    tray.set_state(
                                        match update.state {
                                            mediaa_protocol::TrayStateWire::Idle => {
                                                crate::infra::tray::TrayState::Idle
                                            }
                                            mediaa_protocol::TrayStateWire::Starting => {
                                                crate::infra::tray::TrayState::Starting
                                            }
                                            mediaa_protocol::TrayStateWire::Recording => {
                                                crate::infra::tray::TrayState::Recording
                                            }
                                            mediaa_protocol::TrayStateWire::Paused => {
                                                crate::infra::tray::TrayState::Paused
                                            }
                                            mediaa_protocol::TrayStateWire::Saving => {
                                                crate::infra::tray::TrayState::Saving
                                            }
                                        },
                                        update.progress,
                                    );
                                }
                                Ok(Vec::new())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::TRAY_SHOW_ACK => {
                        self.tray_event(TrayUiEvent::WindowShown, Instant::now());
                        Ok(Vec::new())
                    }
                    mediaa_protocol::TRAY_HIDE_ACK => {
                        self.tray_event(TrayUiEvent::WindowHidden, Instant::now());
                        Ok(Vec::new())
                    }
                    mediaa_protocol::QUIT => {
                        let reason = String::from_utf8_lossy(&request.payload);
                        log::info!("the ui asked to quit: {reason}");
                        self.tray_event(TrayUiEvent::Quit { immediate: true }, Instant::now());
                        Ok(Vec::new())
                    }
                    mediaa_protocol::PICKER_SHOWN => {
                        self.hide_mask();
                        Ok(Vec::new())
                    }
                    mediaa_protocol::backend::PREVIEW_ENSURE => {
                        match mediaa_protocol::backend::decode_preview_request(&request.payload) {
                            Ok(preview) => {
                                if self.previews.is_none() {
                                    match crate::features::preview::PreviewHandle::start(
                                        &self.settings.media_dir,
                                    ) {
                                        Ok(handle) => self.previews = Some(handle),
                                        Err(error) => log::warn!("preview backend: {error:#}"),
                                    }
                                }
                                match self.previews.as_ref() {
                                    Some(handle) => match handle.request(&preview) {
                                        Ok(decision) => {
                                            let state = match decision {
                                                crate::features::preview::store::PreviewDecision::Ready {
                                                    path,
                                                } => mediaa_protocol::backend::PreviewStateWire::Ready {
                                                    path,
                                                },
                                                crate::features::preview::store::PreviewDecision::Pending { .. } => {
                                                    mediaa_protocol::backend::PreviewStateWire::Pending
                                                }
                                            };
                                            let changed =
                                                mediaa_protocol::backend::PreviewChangedWire {
                                                    path: preview.path,
                                                    kind: preview.kind,
                                                    state,
                                                };
                                            Ok(mediaa_protocol::backend::encode_preview_changed(
                                                &changed,
                                            ))
                                        }
                                        Err(error) => Err(format!("{error:#}")),
                                    },
                                    None => Err("preview backend is unavailable".into()),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::LIBRARY_REMOVE => {
                        match mediaa_protocol::backend::decode_library_remove(&request.payload) {
                            Ok(remove) => {
                                for path in &remove.paths {
                                    let path = std::path::Path::new(path);
                                    let result = if remove.permanent {
                                        std::fs::remove_file(path)
                                            .map_err(|error| error.to_string())
                                    } else {
                                        mediaa_platform::files::move_to_trash(path)
                                            .map_err(|error| format!("{error:#}"))
                                    };
                                    if let Err(error) = result {
                                        log::warn!("library remove {}: {error}", path.display());
                                    }
                                }
                                for path in &remove.paths {
                                    if let Some(store) = self.library.as_mut() {
                                        if let Err(error) =
                                            store.delete_path(std::path::Path::new(path))
                                        {
                                            log::warn!("library index remove {path}: {error:#}");
                                        }
                                    }
                                }
                                self.publish_library(remove.paths, false);
                                Ok(Vec::new())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::LIBRARY_RENAME => {
                        match mediaa_protocol::backend::decode_library_rename(&request.payload) {
                            Ok(rename) => {
                                let from = std::path::Path::new(&rename.from);
                                let to = std::path::Path::new(&rename.to);
                                match std::fs::rename(from, to) {
                                    Ok(()) => {
                                        if let Some(store) = self.library.as_mut() {
                                            let _ = store.delete_path(from);
                                            if let Err(error) = store.upsert_path(to) {
                                                log::warn!("library index rename {error:#}");
                                            }
                                        }
                                        self.publish_library(vec![rename.to], false);
                                        Ok(Vec::new())
                                    }
                                    Err(error) => Err(format!("{error}")),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::LIBRARY_EDIT => {
                        match mediaa_protocol::backend::decode_library_edit(&request.payload) {
                            Ok(edit) => {
                                let result = match self.library.as_mut() {
                                    Some(store) => store.edit(&edit).map(|_| ()),
                                    None => Ok(()),
                                };
                                match result {
                                    Ok(()) => {
                                        self.publish_library(Vec::new(), false);
                                        Ok(Vec::new())
                                    }
                                    Err(error) => Err(format!("{error:#}")),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::LIBRARY_ADD => {
                        match mediaa_protocol::backend::decode_library_add(&request.payload) {
                            Ok(paths) => {
                                let paths: Vec<std::path::PathBuf> =
                                    paths.into_iter().map(std::path::PathBuf::from).collect();
                                self.index_library_paths(&paths);
                                Ok(Vec::new())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::AGENT_SEND => {
                        match mediaa_protocol::backend::decode_agent_send(&request.payload) {
                            Ok(send) => match self.send_agent_message(send) {
                                Ok(chat) => Ok(mediaa_protocol::backend::encode_agent_send_result(
                                    &mediaa_protocol::backend::AgentSendResultWire { chat },
                                )),
                                Err(error) => Err(error),
                            },
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::AGENT_OP => {
                        match mediaa_protocol::backend::decode_agent_op(&request.payload) {
                            Ok(op) => self.agent_op(op).map(|_| Vec::new()),
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::AGENT_TASK => {
                        match mediaa_protocol::backend::decode_agent_task(&request.payload) {
                            Ok(task) => self.agent_task_details(task.id),
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::AGENT_ANALYSE => {
                        match mediaa_protocol::backend::decode_agent_analyse(&request.payload) {
                            Ok(analyse) => self.agent_analyse(analyse).map(|_| Vec::new()),
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::AGENT_NAMES => {
                        match mediaa_protocol::backend::decode_agent_names(&request.payload) {
                            Ok(names) => self.agent_names(names).map(|_| Vec::new()),
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::AGENT_STOP => {
                        match mediaa_protocol::backend::decode_agent_task(&request.payload) {
                            Ok(task) => self.agent_stop(task.id),
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::AGENT_STEER => {
                        match mediaa_protocol::backend::decode_agent_steer(&request.payload) {
                            Ok(steer) => self.agent_steer(steer).map(|_| Vec::new()),
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::CAPTURE_REQUEST => {
                        match mediaa_protocol::backend::decode_capture_request(&request.payload) {
                            Ok(capture) => crate::features::capture::capture_target_file(
                                &self.settings.media_dir,
                                capture.target,
                            )
                            .map(|source| mediaa_protocol::backend::encode_capture_source(&source)),
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::SCREENSHOT_COMMIT => {
                        match mediaa_protocol::backend::decode_screenshot_commit(&request.payload) {
                            Ok(commit) => {
                                if self.clipboard.is_none() {
                                    self.clipboard =
                                        Some(crate::features::clipboard::ClipboardService::new(
                                            Wake::default(),
                                        ));
                                }
                                let result = match self.clipboard.as_mut() {
                                    Some(service) => {
                                        let handle = service.handle();
                                        crate::features::capture::commit_screenshot(
                                            &commit, &handle,
                                        )
                                    }
                                    None => Err("clipboard is unavailable".into()),
                                };
                                result.map(|result| {
                                    mediaa_protocol::backend::encode_shot_result(&result)
                                })
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::CLIPBOARD_COPY => {
                        match mediaa_protocol::backend::decode_clipboard(&request.payload) {
                            Ok(copy) => {
                                if self.clipboard.is_none() {
                                    self.clipboard =
                                        Some(crate::features::clipboard::ClipboardService::new(
                                            Wake::default(),
                                        ));
                                }
                                let result = (|| -> Result<(), String> {
                                    let Some(service) = self.clipboard.as_mut() else {
                                        return Err("clipboard is unavailable".into());
                                    };
                                    let handle = service.handle();
                                    if let Some(text) = copy.text {
                                        handle
                                            .copy_text(
                                                text,
                                                crate::features::clipboard::Purpose::Silent,
                                            )
                                            .map_err(|error| format!("{error:#}"))?;
                                    }
                                    if !copy.files.is_empty() {
                                        let files = copy
                                            .files
                                            .into_iter()
                                            .map(std::path::PathBuf::from)
                                            .collect();
                                        handle
                                            .copy_files(
                                                files,
                                                crate::features::clipboard::Purpose::Silent,
                                            )
                                            .map_err(|error| format!("{error:#}"))?;
                                    }
                                    if let Some(image) = copy.image {
                                        let image = image::open(&image)
                                            .map_err(|error| error.to_string())?;
                                        handle
                                            .copy_image_wait(mediaa_core::Frame::from_image(
                                                image.to_rgba8(),
                                            ))
                                            .map_err(|error| error.to_string())?;
                                    }
                                    Ok(())
                                })();
                                result.map(|_| Vec::new())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::CONVERSION_START => {
                        match mediaa_protocol::backend::decode_conversion_start(&request.payload) {
                            Ok(start) => {
                                self.conversion.set_inputs(
                                    start
                                        .inputs
                                        .into_iter()
                                        .map(std::path::PathBuf::from)
                                        .collect(),
                                );
                                self.conversion.target = Some(start.target);
                                self.conversion.settings = start.settings;
                                self.conversion.folder = start.folder.map(std::path::PathBuf::from);
                                let dir = crate::features::library::library_dir(&self.settings);
                                match self.conversion.start_from_ui(&self.conversion_wake, &dir) {
                                    Some(error) => Err(error),
                                    None => Ok(Vec::new()),
                                }
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::backend::CONVERSION_CANCEL => {
                        self.conversion.cancel();
                        Ok(Vec::new())
                    }
                    mediaa_protocol::backend::LIBRARY_QUERY => {
                        match mediaa_protocol::backend::decode_library_query(&request.payload) {
                            Ok(query) => match self.library.as_ref() {
                                Some(store) => match store.query(&query) {
                                    Ok(page) => {
                                        Ok(mediaa_protocol::backend::encode_library_page(&page))
                                    }
                                    Err(error) => Err(format!("{error:#}")),
                                },
                                None => Ok(mediaa_protocol::backend::encode_library_page(
                                    &mediaa_protocol::backend::LibraryPageWire {
                                        entries: Vec::new(),
                                        next_cursor: None,
                                        version: 0,
                                    },
                                )),
                            },
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::PICKER_STARTING => {
                        self.show_mask();
                        Ok(Vec::new())
                    }
                    mediaa_protocol::SET_SETTINGS => {
                        match mediaa_protocol::decode_settings(&request.payload) {
                            Ok(settings) => {
                                self.apply_settings(settings);
                                Ok(Vec::new())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    mediaa_protocol::RECORDING_START
                    | mediaa_protocol::RECORDING_STOP
                    | mediaa_protocol::RECORDING_TOGGLE
                    | mediaa_protocol::RECORDING_CANCEL_START
                    | mediaa_protocol::RECORDING_CANCEL_FINALIZE => {
                        match mediaa_protocol::decode_recording_command(&request.payload) {
                            Ok(command) => {
                                self.command_recording(command);
                                Ok(Vec::new())
                            }
                            Err(error) => Err(error),
                        }
                    }
                    _ => Err(format!("unsupported command: {}", request.name)),
                };
                let ok = result.is_ok();
                let frame = self.sync.complete_command(request.id, result);
                self.send_frame(frame);
                if ok {
                    self.flush_sync();
                }
            }
        }
        if subscribe {
            self.flush_pending_events();
        }
    }
}
