use crate::services::{LibraryHandle, NoticeRing};
use crate::state::StateStore;
use mediaa2_core::annotate::Shape;
use mediaa2_core::formats::ImageFormat;
use mediaa2_core::frame::Frame;
use mediaa2_core::geometry::Rect;
use mediaa2_core::naming;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::paths;
use mediaa2_core::screen::{MonitorInfo, WindowInfo, WindowState};
use mediaa2_core::settings::AfterCapture;
use mediaa2_media::screenshot;
use mediaa2_platform::{clipboard, cursor, screen};
use mediaa2_protocol::state::PickerState;
use mediaa2_protocol::{CommandError, CommandOk, ErrorCode, ShotResult};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

const SCREENSHOT_TITLE: &str = "Screenshot";

#[derive(Clone)]
pub struct ScreenshotHandle {
    state: Arc<StateStore>,
    library: LibraryHandle,
    notices: NoticeRing,
    sessions: Arc<AtomicU64>,
}

impl ScreenshotHandle {
    pub fn new(state: Arc<StateStore>, library: LibraryHandle, notices: NoticeRing) -> Self {
        Self {
            state,
            library,
            notices,
            sessions: Arc::new(AtomicU64::new(1)),
        }
    }

    pub async fn capture_monitor(
        &self,
        monitor: MonitorInfo,
        after: AfterCapture,
    ) -> Result<CommandOk, CommandError> {
        let media_dir = self.media_dir();
        let frame = tokio::task::spawn_blocking(move || {
            let started = std::time::Instant::now();
            let frame = screen::capture_monitor(monitor.id).map_err(unavailable)?;
            log::info!(
                "screenshot: monitor captured {}x{} in {:?}",
                frame.width,
                frame.height,
                started.elapsed()
            );
            Ok::<_, CommandError>(frame)
        })
        .await
        .map_err(internal)??;
        let shot = spawn_shot(
            Arc::clone(&self.state),
            self.library.clone(),
            self.notices.clone(),
            frame,
            media_dir,
            after,
        )?;
        Ok(CommandOk::Shot(shot))
    }

    pub async fn capture_window(
        &self,
        window: WindowInfo,
        after: AfterCapture,
    ) -> Result<CommandOk, CommandError> {
        let status = screen::window_status(window.id, window.pid, &window.app);
        if status != WindowState::Available {
            return Err(CommandError {
                code: ErrorCode::Unavailable,
                message: window_message(status),
            });
        }
        let title = if window.title.trim().is_empty() {
            SCREENSHOT_TITLE.to_string()
        } else {
            window.title.clone()
        };
        let media_dir = self.media_dir();
        let state = Arc::clone(&self.state);
        let library = self.library.clone();
        let notices = self.notices.clone();
        let shot = tokio::task::spawn_blocking(move || {
            let frame = screen::capture_window(window.id).map_err(unavailable)?;
            deliver(
                &state,
                &library,
                &notices,
                frame,
                &media_dir,
                ShotRequest {
                    format: ImageFormat::Png,
                    title: &title,
                    action: after,
                    output: None,
                },
            )
        })
        .await
        .map_err(internal)??;
        Ok(CommandOk::Shot(shot))
    }

    pub async fn start_picker(&self) -> Result<CommandOk, CommandError> {
        if self.state.snapshot().picker.is_some() {
            return Err(busy("picker is already open"));
        }
        let session = self.sessions.fetch_add(1, Ordering::SeqCst);
        let work = paths::work_dir();
        let state = Arc::clone(&self.state);
        let captured = tokio::task::spawn_blocking(move || capture_picker(session, &work))
            .await
            .map_err(internal)??;
        let picker = PickerState {
            session,
            monitor: captured.monitor,
            image: captured.path,
            scale: captured.scale,
        };
        let published = picker.clone();
        let mut accepted = false;
        state.update(|current| {
            if current.picker.is_none() {
                current.picker = Some(published.clone());
                accepted = true;
            }
        });
        if !accepted {
            let _ = std::fs::remove_file(picker.image);
            return Err(busy("picker is already open"));
        }
        Ok(CommandOk::PickerStarted { session })
    }

    pub async fn picker_commit(
        &self,
        session: u64,
        selection: Rect,
        shapes: Vec<Shape>,
        format: ImageFormat,
        action: AfterCapture,
        output: Option<PathBuf>,
    ) -> Result<CommandOk, CommandError> {
        let picker = self.picker(session)?;
        if selection.is_empty() {
            return Err(invalid("the selection is empty"));
        }
        let media_dir = self.media_dir();
        let state = Arc::clone(&self.state);
        let library = self.library.clone();
        let notices = self.notices.clone();
        let image = picker.image;
        let shot = tokio::task::spawn_blocking(move || {
            let frame = load_frame(&image)?;
            let composed = screenshot::compose(&frame, selection, &shapes);
            deliver(
                &state,
                &library,
                &notices,
                composed,
                &media_dir,
                ShotRequest {
                    format,
                    title: SCREENSHOT_TITLE,
                    action,
                    output,
                },
            )
        })
        .await
        .map_err(internal)??;
        self.release(session);
        Ok(CommandOk::Shot(shot))
    }

    pub async fn picker_cancel(&self, session: u64) -> Result<CommandOk, CommandError> {
        self.release(session);
        Ok(CommandOk::Unit)
    }

    pub async fn picker_monitor(
        &self,
        session: u64,
        index: u32,
    ) -> Result<CommandOk, CommandError> {
        let _ = self.picker(session)?;
        let work = paths::work_dir();
        let state = Arc::clone(&self.state);
        let (monitor, path, width, height) = tokio::task::spawn_blocking(move || {
            let monitors = screen::monitors().map_err(unavailable)?;
            let monitor = monitors
                .get(index as usize)
                .cloned()
                .ok_or_else(|| invalid("the monitor is not available"))?;
            let frame = screen::capture_monitor(monitor.id).map_err(unavailable)?;
            let path = write_picker_frame(session, &work, &frame)?;
            Ok::<_, CommandError>((monitor, path, frame.width, frame.height))
        })
        .await
        .map_err(internal)??;
        let scale = monitor.scale;
        let update_path = path.clone();
        let mut accepted = false;
        let mut previous: Option<PathBuf> = None;
        state.update(|current| {
            if let Some(picker) = current
                .picker
                .as_mut()
                .filter(|picker| picker.session == session)
            {
                previous = Some(std::mem::replace(&mut picker.image, update_path));
                picker.monitor = monitor;
                picker.scale = scale;
                accepted = true;
            }
        });
        if !accepted {
            let _ = std::fs::remove_file(&path);
            return Err(CommandError {
                code: ErrorCode::NotFound,
                message: "picker session is closed".into(),
            });
        }
        if let Some(previous) = previous {
            if previous != path {
                let _ = std::fs::remove_file(previous);
            }
        }
        Ok(CommandOk::PickerFrame {
            path,
            width,
            height,
        })
    }

    fn media_dir(&self) -> PathBuf {
        self.state.snapshot().settings.media_dir.clone()
    }

    fn picker(&self, session: u64) -> Result<PickerState, CommandError> {
        let snapshot = self.state.snapshot();
        match &snapshot.picker {
            Some(picker) if picker.session == session => Ok(picker.clone()),
            _ => Err(CommandError {
                code: ErrorCode::NotFound,
                message: "picker session is closed".into(),
            }),
        }
    }

    fn release(&self, session: u64) {
        let mut removed: Option<PathBuf> = None;
        self.state.update(|current| {
            if current
                .picker
                .as_ref()
                .is_some_and(|picker| picker.session == session)
            {
                removed = current.picker.take().map(|picker| picker.image);
            }
        });
        if let Some(image) = removed {
            let _ = std::fs::remove_file(image);
        }
    }
}

struct CapturedPicker {
    monitor: MonitorInfo,
    path: PathBuf,
    scale: f32,
}

fn capture_picker(session: u64, work: &Path) -> Result<CapturedPicker, CommandError> {
    let started = std::time::Instant::now();
    let monitors = screen::monitors().map_err(unavailable)?;
    log::info!("picker: monitors in {:?}", started.elapsed());
    let monitor = monitor_for_picker(&monitors)?;
    let captured = std::time::Instant::now();
    let frame = screen::capture_monitor(monitor.id).map_err(unavailable)?;
    log::info!(
        "picker: capture {}x{} in {:?}",
        frame.width,
        frame.height,
        captured.elapsed()
    );
    let written = std::time::Instant::now();
    let path = write_picker_frame(session, work, &frame)?;
    log::info!("picker: frame written in {:?}", written.elapsed());
    log::info!("picker: capture total in {:?}", started.elapsed());
    Ok(CapturedPicker {
        scale: monitor.scale,
        monitor,
        path,
    })
}

fn monitor_for_picker(monitors: &[MonitorInfo]) -> Result<MonitorInfo, CommandError> {
    if let Some((x, y)) = cursor::position() {
        if let Some(id) = screen::monitor_at(x, y) {
            if let Some(monitor) = monitors.iter().find(|monitor| monitor.id == id) {
                return Ok(monitor.clone());
            }
        }
    }
    monitors.first().cloned().ok_or_else(|| CommandError {
        code: ErrorCode::Unavailable,
        message: "no monitors are available".into(),
    })
}

const PICKER_MAGIC: &[u8; 4] = b"MPK1";

fn write_picker_frame(session: u64, work: &Path, frame: &Frame) -> Result<PathBuf, CommandError> {
    std::fs::create_dir_all(work).map_err(internal)?;
    let path = work.join(format!("picker-{session}.rgba"));
    write_picker_rgba(&path, frame).map_err(internal)?;
    Ok(path)
}

fn write_picker_rgba(path: &Path, frame: &Frame) -> anyhow::Result<()> {
    let mut bytes = Vec::with_capacity(16 + frame.data.len());
    bytes.extend_from_slice(PICKER_MAGIC);
    bytes.extend_from_slice(&frame.width.to_le_bytes());
    bytes.extend_from_slice(&frame.height.to_le_bytes());
    bytes.extend_from_slice(&(frame.width * 4).to_le_bytes());
    bytes.extend_from_slice(&frame.data);
    std::fs::write(path, bytes)?;
    Ok(())
}

fn read_picker_rgba(path: &Path) -> anyhow::Result<Frame> {
    let bytes = std::fs::read(path)?;
    anyhow::ensure!(
        bytes.len() >= 16 && &bytes[..4] == PICKER_MAGIC,
        "bad picker frame"
    );
    let width = u32::from_le_bytes(bytes[4..8].try_into().expect("width"));
    let height = u32::from_le_bytes(bytes[8..12].try_into().expect("height"));
    let stride = u32::from_le_bytes(bytes[12..16].try_into().expect("stride"));
    anyhow::ensure!(stride == width * 4, "bad picker stride");
    let data = bytes[16..].to_vec();
    anyhow::ensure!(
        data.len() == (width as usize) * (height as usize) * 4,
        "bad picker frame size"
    );
    Ok(Frame::new(width, height, data))
}

fn load_frame(path: &Path) -> Result<Frame, CommandError> {
    if !path.is_file() {
        return Err(CommandError {
            code: ErrorCode::NotFound,
            message: format!("the picker frame is missing: {}", path.display()),
        });
    }
    if path.extension().and_then(|ext| ext.to_str()) == Some("rgba") {
        return read_picker_rgba(path).map_err(|error| CommandError {
            code: ErrorCode::Internal,
            message: format!("{}: {error}", path.display()),
        });
    }
    let image = image::open(path).map_err(|error| CommandError {
        code: ErrorCode::Internal,
        message: format!("{}: {error}", path.display()),
    })?;
    Ok(Frame::from_image(image.to_rgba8()))
}

struct ShotRequest<'a> {
    format: ImageFormat,
    title: &'a str,
    action: AfterCapture,
    output: Option<PathBuf>,
}

fn spawn_shot(
    state: Arc<StateStore>,
    library: LibraryHandle,
    notices: NoticeRing,
    frame: Frame,
    media_dir: PathBuf,
    after: AfterCapture,
) -> Result<ShotResult, CommandError> {
    let size = format!("{}×{}", frame.width, frame.height);
    let path = if after.saves() {
        std::fs::create_dir_all(&media_dir).map_err(internal)?;
        Some(naming::media_path(
            &media_dir,
            SCREENSHOT_TITLE,
            chrono::Local::now(),
            ImageFormat::Png.extension(),
        ))
    } else {
        None
    };
    let copied = after.copies();
    let background_path = path.clone();
    std::thread::spawn(move || {
        finish_shot(&state, &library, &notices, frame, background_path, copied);
    });
    Ok(ShotResult { path, size, copied })
}

fn finish_shot(
    state: &StateStore,
    library: &LibraryHandle,
    notices: &NoticeRing,
    frame: Frame,
    path: Option<PathBuf>,
    copies: bool,
) {
    if let Some(path) = &path {
        if let Err(error) = screenshot::save(&frame, ImageFormat::Png, path) {
            notices.push(
                state,
                NoticeLevel::Error,
                "Screenshot",
                format!("{error:#}"),
            );
            return;
        }
        match library.add_paths(std::slice::from_ref(path)) {
            Ok(()) => {
                let revision = library.revision();
                state.update(|current| current.library_revision = revision);
            }
            Err(error) => notices.push(state, NoticeLevel::Error, "Library", format!("{error:#}")),
        }
    }
    if copies {
        if let Err(error) = clipboard::copy_image(frame) {
            notices.push(
                state,
                NoticeLevel::Warning,
                "Copy failed",
                format!("{error:#}"),
            );
        }
    }
}

fn deliver(
    state: &StateStore,
    library: &LibraryHandle,
    notices: &NoticeRing,
    frame: Frame,
    media_dir: &Path,
    request: ShotRequest<'_>,
) -> Result<ShotResult, CommandError> {
    let ShotRequest {
        format,
        title,
        action,
        output,
    } = request;
    let size = format!("{}×{}", frame.width, frame.height);
    let path = match output {
        Some(output) => Some(output),
        None if action.saves() => {
            std::fs::create_dir_all(media_dir).map_err(internal)?;
            Some(naming::media_path(
                media_dir,
                title,
                chrono::Local::now(),
                format.extension(),
            ))
        }
        None => None,
    };
    if let Some(path) = path.as_deref() {
        screenshot::save(&frame, format, path).map_err(internal)?;
    }
    let copies = action.copies();
    let copy_error = if copies {
        clipboard::copy_image(frame).err()
    } else {
        None
    };
    if let Some(path) = &path {
        match library.add_paths(std::slice::from_ref(path)) {
            Ok(()) => {
                let revision = library.revision();
                state.update(|current| current.library_revision = revision);
            }
            Err(error) => notices.push(state, NoticeLevel::Error, "Library", format!("{error:#}")),
        }
    }
    match copy_error {
        Some(error) if path.is_none() => Err(CommandError {
            code: ErrorCode::Internal,
            message: format!("clipboard: {error:#}"),
        }),
        Some(error) => {
            notices.push(
                state,
                NoticeLevel::Warning,
                "Copy failed",
                format!("{error:#}"),
            );
            Ok(ShotResult {
                path,
                size,
                copied: false,
            })
        }
        None => Ok(ShotResult {
            path,
            size,
            copied: copies,
        }),
    }
}

fn window_message(state: WindowState) -> String {
    match state {
        WindowState::Available => "the window is available".into(),
        WindowState::Minimized => "the window is minimized".into(),
        WindowState::Hidden => "the window is hidden".into(),
        WindowState::Missing => "the window was closed".into(),
        WindowState::Mismatch => "the window belongs to another process".into(),
    }
}

fn unavailable(error: impl std::fmt::Display) -> CommandError {
    CommandError {
        code: ErrorCode::Unavailable,
        message: format!("{error:#}"),
    }
}

fn internal(error: impl std::fmt::Display) -> CommandError {
    CommandError {
        code: ErrorCode::Internal,
        message: format!("{error:#}"),
    }
}

fn invalid(message: &str) -> CommandError {
    CommandError {
        code: ErrorCode::Invalid,
        message: message.into(),
    }
}

fn busy(message: &str) -> CommandError {
    CommandError {
        code: ErrorCode::Busy,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::test_state;
    use mediaa2_core::formats::MediaKind;
    use mediaa2_core::library::LibraryEntry;
    use mediaa2_core::query::LibraryQuery;
    use mediaa2_core::screen::{CaptureWindowId, MonitorId};

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(1),
            device: r"\\.\DISPLAY1".into(),
            index: 0,
            name: "DELL U2720Q".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: true,
        }
    }

    fn setup(dir: &Path) -> (Arc<StateStore>, ScreenshotHandle, LibraryHandle) {
        let mut initial = test_state();
        initial.settings.media_dir = dir.join("media");
        let state = Arc::new(StateStore::new(initial));
        let library = LibraryHandle::open(dir.join("media")).unwrap();
        let handle = ScreenshotHandle::new(Arc::clone(&state), library.clone(), NoticeRing::new());
        (state, handle, library)
    }

    fn write_picker_image(dir: &Path, session: u64, frame: &Frame) -> PathBuf {
        let path = dir.join(format!("picker-{session}.png"));
        screenshot::save(frame, ImageFormat::Png, &path).unwrap();
        path
    }

    fn entries(library: &LibraryHandle) -> Vec<LibraryEntry> {
        library.query(&LibraryQuery::default()).unwrap().entries
    }

    #[tokio::test]
    async fn a_saved_frame_is_written_to_the_media_dir_and_indexed() {
        let dir = tempfile::tempdir().unwrap();
        let (state, handle, library) = setup(dir.path());
        let media = state.snapshot().settings.media_dir.clone();
        let frame = Frame::filled(64, 32, [10, 20, 30, 255]);
        let result = deliver(
            &state,
            &library,
            &handle.notices,
            frame,
            &media,
            ShotRequest {
                format: ImageFormat::Png,
                title: SCREENSHOT_TITLE,
                action: AfterCapture::Save,
                output: None,
            },
        )
        .unwrap();
        assert_eq!(result.size, "64×32");
        assert!(!result.copied);
        let path = result.path.expect("saved path");
        assert!(path.starts_with(&media));
        let saved = image::open(&path).unwrap().to_rgba8();
        assert_eq!(saved.dimensions(), (64, 32));
        assert_eq!(saved.get_pixel(0, 0).0, [10, 20, 30, 255]);
        let entries = entries(&library);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, MediaKind::Image);
        assert_eq!(entries[0].path, path);
        assert_eq!(state.snapshot().library_revision, 1);
    }

    #[test]
    fn a_shot_is_returned_before_the_file_is_written() {
        let dir = tempfile::tempdir().unwrap();
        let (state, handle, library) = setup(dir.path());
        let media = state.snapshot().settings.media_dir.clone();
        let frame = Frame::filled(64, 32, [1, 2, 3, 255]);
        let shot = spawn_shot(
            Arc::clone(&state),
            library.clone(),
            handle.notices.clone(),
            frame,
            media,
            AfterCapture::Save,
        )
        .unwrap();
        let path = shot.path.expect("saved path");
        assert_eq!(shot.size, "64×32");
        assert!(!shot.copied);
        assert!(
            !path.exists(),
            "the result must return before the file is written"
        );
        for _ in 0..100 {
            if path.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(path.exists());
        assert_eq!(entries(&library).len(), 1);
        assert_eq!(state.snapshot().library_revision, 1);
    }

    #[tokio::test]
    async fn a_copy_only_capture_does_not_create_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _handle, library) = setup(dir.path());
        let media = state.snapshot().settings.media_dir.clone();
        let frame = Frame::filled(8, 8, [0, 0, 0, 255]);
        let result = deliver(
            &state,
            &library,
            &NoticeRing::new(),
            frame,
            &media,
            ShotRequest {
                format: ImageFormat::Png,
                title: SCREENSHOT_TITLE,
                action: AfterCapture::Copy,
                output: None,
            },
        )
        .unwrap();
        assert!(result.path.is_none());
        assert!(result.copied);
        assert!(entries(&library).is_empty());
        assert_eq!(state.snapshot().library_revision, 0);
    }

    #[tokio::test]
    async fn picker_commit_crops_the_frame_and_releases_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let (state, handle, library) = setup(dir.path());
        let source = write_picker_image(dir.path(), 7, &Frame::filled(80, 60, [5, 6, 7, 255]));
        state.update(|current| {
            current.picker = Some(PickerState {
                session: 7,
                monitor: monitor(),
                image: source.clone(),
                scale: 1.0,
            })
        });
        let result = handle
            .picker_commit(
                7,
                Rect::new(10, 5, 20, 10),
                Vec::new(),
                ImageFormat::Png,
                AfterCapture::Save,
                None,
            )
            .await
            .unwrap();
        let CommandOk::Shot(shot) = result else {
            panic!("expected a shot result");
        };
        assert_eq!(shot.size, "20×10");
        assert!(!shot.copied);
        let path = shot.path.expect("saved path");
        let saved = image::open(&path).unwrap().to_rgba8();
        assert_eq!(saved.dimensions(), (20, 10));
        assert_eq!(saved.get_pixel(0, 0).0, [5, 6, 7, 255]);
        assert!(!source.exists());
        assert!(state.snapshot().picker.is_none());
        assert_eq!(entries(&library).len(), 1);
    }

    #[tokio::test]
    async fn picker_commit_writes_to_an_explicit_output() {
        let dir = tempfile::tempdir().unwrap();
        let (state, handle, _library) = setup(dir.path());
        let source = write_picker_image(dir.path(), 3, &Frame::filled(40, 30, [1, 2, 3, 255]));
        state.update(|current| {
            current.picker = Some(PickerState {
                session: 3,
                monitor: monitor(),
                image: source.clone(),
                scale: 1.0,
            })
        });
        let output = dir.path().join("chosen.webp");
        let result = handle
            .picker_commit(
                3,
                Rect::new(0, 0, 10, 10),
                Vec::new(),
                ImageFormat::Webp,
                AfterCapture::Save,
                Some(output.clone()),
            )
            .await
            .unwrap();
        let CommandOk::Shot(shot) = result else {
            panic!("expected a shot result");
        };
        assert_eq!(shot.path.as_deref(), Some(output.as_path()));
        assert!(output.exists());
        assert_eq!(
            image::open(&output).unwrap().to_rgba8().dimensions(),
            (10, 10)
        );
        assert!(!source.exists());
    }

    #[tokio::test]
    async fn commit_with_a_stale_session_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (_state, handle, _library) = setup(dir.path());
        let error = handle
            .picker_commit(
                9,
                Rect::new(0, 0, 4, 4),
                Vec::new(),
                ImageFormat::Png,
                AfterCapture::Save,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn an_empty_selection_keeps_the_session_and_the_frame() {
        let dir = tempfile::tempdir().unwrap();
        let (state, handle, _library) = setup(dir.path());
        let source = write_picker_image(dir.path(), 5, &Frame::filled(20, 20, [0, 0, 0, 255]));
        state.update(|current| {
            current.picker = Some(PickerState {
                session: 5,
                monitor: monitor(),
                image: source.clone(),
                scale: 1.0,
            })
        });
        let error = handle
            .picker_commit(
                5,
                Rect::new(4, 4, 0, 0),
                Vec::new(),
                ImageFormat::Png,
                AfterCapture::Save,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Invalid);
        assert!(state.snapshot().picker.is_some());
        assert!(source.exists());
    }

    #[tokio::test]
    async fn picker_cancel_clears_the_session_and_removes_the_frame() {
        let dir = tempfile::tempdir().unwrap();
        let (state, handle, _library) = setup(dir.path());
        let source = write_picker_image(dir.path(), 11, &Frame::filled(20, 20, [0, 0, 0, 255]));
        state.update(|current| {
            current.picker = Some(PickerState {
                session: 11,
                monitor: monitor(),
                image: source.clone(),
                scale: 1.0,
            })
        });
        assert_eq!(handle.picker_cancel(11).await.unwrap(), CommandOk::Unit);
        assert!(state.snapshot().picker.is_none());
        assert!(!source.exists());
        assert_eq!(handle.picker_cancel(11).await.unwrap(), CommandOk::Unit);
        assert_eq!(handle.picker_cancel(12).await.unwrap(), CommandOk::Unit);
    }

    #[tokio::test]
    async fn monitor_switch_with_a_stale_session_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let (_state, handle, _library) = setup(dir.path());
        let error = handle.picker_monitor(9, 0).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::NotFound);
    }

    #[tokio::test]
    async fn a_missing_window_is_unavailable() {
        let dir = tempfile::tempdir().unwrap();
        let (_state, handle, _library) = setup(dir.path());
        let window = WindowInfo {
            id: CaptureWindowId::new(0),
            title: "Gone".into(),
            app: "app".into(),
            pid: 0,
            width: 100,
            height: 100,
        };
        let error = handle
            .capture_window(window, AfterCapture::Save)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Unavailable);
    }

    #[tokio::test]
    #[ignore = "captures the real desktop"]
    async fn capturing_a_monitor_writes_a_file() {
        mediaa2_core::logging::init_named(
            &std::env::temp_dir(),
            "mediaa2-shot-timing.log",
            log::LevelFilter::Info,
        );
        let Ok(monitors) = screen::monitors() else {
            return;
        };
        let Some(monitor) = monitors.into_iter().next() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (state, handle, library) = setup(dir.path());
        let result = handle
            .capture_monitor(monitor, AfterCapture::CopyAndSave)
            .await
            .unwrap();
        let CommandOk::Shot(shot) = result else {
            panic!("expected a shot result");
        };
        let path = shot.path.expect("saved path");
        for _ in 0..100 {
            if path.is_file() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        assert!(path.exists());
        assert_eq!(entries(&library).len(), 1);
        assert_eq!(state.snapshot().library_revision, 1);
    }
}
