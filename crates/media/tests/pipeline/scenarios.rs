use super::*;
use std::cell::RefCell;
use std::process::Command;

thread_local! {
    static VIDEO_FAILURES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

struct VideoCheck;

impl Drop for VideoCheck {
    fn drop(&mut self) {
        let failures = VIDEO_FAILURES.with(|failures| failures.borrow_mut().split_off(0));
        assert!(failures.is_empty(), "{}", failures.join("; "));
    }
}

struct DocSpan {
    from: f32,
    to: f32,
    text: &'static str,
}

struct DocColumn {
    name: &'static str,
    expected: PathBuf,
    actual: PathBuf,
    state: Vec<DocSpan>,
    expectation: Vec<DocSpan>,
}

fn span(from: f32, to: f32, text: &'static str) -> DocSpan {
    DocSpan { from, to, text }
}

fn drawtext(font: &str, text: &str, x: i32, y: &str, size: i32, from: f32, to: f32) -> String {
    let text = text
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace(':', "\\:");
    format!(
        "drawtext=fontfile='{font}':text='{text}':x={x}:y={y}:fontsize={size}:fontcolor=white:box=1:boxcolor=black@0.65:boxborderw=5:enable='between(t,{from},{to})'"
    )
}

fn write_comparative_doc(test: &str, columns: &[DocColumn]) {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/comparative-docs");
    std::fs::create_dir_all(&dir).unwrap();
    let font_path = dir.join("ubuntu.ttf");
    if !font_path.exists() {
        std::fs::write(&font_path, mediaa_render::annotate::FONT).unwrap();
    }
    let font = font_path
        .to_string_lossy()
        .replace('\\', "/")
        .replace(':', "\\:");
    let mut args = vec!["-y".to_string()];
    for column in columns {
        args.push("-i".into());
        args.push(column.expected.to_string_lossy().into_owned());
        args.push("-i".into());
        args.push(column.actual.to_string_lossy().into_owned());
    }

    let mut filters = Vec::new();
    let mut columns_out = Vec::new();
    for (index, column) in columns.iter().enumerate() {
        let expected = index * 2;
        let actual = expected + 1;
        let mut expected_filter = format!(
            "[{expected}:v]scale=640:360:force_original_aspect_ratio=decrease,pad=640:360:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,{}",
            drawtext(&font, "ОЖИДАНИЕ", 10, "8", 22, 0.0, 30.0)
        );
        expected_filter.push(',');
        expected_filter.push_str(&drawtext(&font, column.name, 10, "h-72", 22, 0.0, 30.0));
        for item in &column.state {
            expected_filter.push(',');
            expected_filter.push_str(&drawtext(
                &font, item.text, 10, "h-42", 20, item.from, item.to,
            ));
        }
        for item in &column.expectation {
            expected_filter.push(',');
            expected_filter.push_str(&drawtext(
                &font, item.text, 10, "h-20", 18, item.from, item.to,
            ));
        }
        expected_filter.push_str(&format!("[e{index}]"));
        filters.push(expected_filter);
        filters.push(format!(
            "[{actual}:v]scale=640:360:force_original_aspect_ratio=decrease,pad=640:360:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,{},{}[a{index}]",
            drawtext(&font, "ФАКТ", 10, "8", 22, 0.0, 30.0),
            drawtext(&font, column.name, 10, "h-24", 20, 0.0, 30.0)
        ));
        filters.push(format!("[e{index}][a{index}]vstack=inputs=2[c{index}]"));
        columns_out.push(format!("[c{index}]"));
    }
    let count = columns.len();
    let out_label = if count == 1 {
        "[c0]".to_string()
    } else {
        filters.push(format!(
            "{}hstack=inputs={count}[out]",
            columns_out.concat()
        ));
        "[out]".to_string()
    };
    let output = dir.join(format!("{test}.mp4"));
    args.push("-filter_complex".into());
    args.push(filters.join(";"));
    args.extend([
        "-map".into(),
        out_label,
        "-t".into(),
        "30".into(),
        "-r".into(),
        "10".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        output.to_string_lossy().into_owned(),
    ]);
    let status = Command::new("ffmpeg").args(&args).status();
    match status {
        Ok(status) if status.success() => {
            eprintln!("comparative document: {}", output.display());
        }
        Ok(status) => eprintln!("comparative document failed: {status}"),
        Err(error) => eprintln!("comparative document skipped: {error}"),
    }
}

fn decode_mp4(path: &Path, width: u32) -> Vec<Frame> {
    FrameExtractor::new(path.to_string_lossy().into_owned())
        .pixel(PixelLayout::Rgba32)
        .width(width & !1)
        .frames()
        .unwrap()
        .map(|frame| {
            let frame = frame.unwrap();
            Frame::new(frame.width(), frame.height(), frame.into_vec())
        })
        .collect()
}

fn signature(frame: &Frame) -> Vec<u8> {
    frame.resized(8, 8).data
}

fn video_difference(a: &[Frame], b: &[Frame]) -> f32 {
    let samples = 12;
    let a = (0..samples)
        .map(|i| {
            let index = i * (a.len() - 1) / (samples - 1);
            signature(&a[index])
        })
        .collect::<Vec<_>>();
    let b = (0..samples)
        .map(|i| {
            let index = i * (b.len() - 1) / (samples - 1);
            signature(&b[index])
        })
        .collect::<Vec<_>>();
    let total: f32 = a
        .iter()
        .zip(&b)
        .map(|(a, b)| {
            let sum: u32 = a
                .iter()
                .zip(b)
                .map(|(a, b)| u32::from(a.abs_diff(*b)))
                .sum();
            sum as f32 / a.len() as f32
        })
        .sum();
    total / a.len() as f32
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots/videos")
        .join(name)
}

fn assert_mp4_snapshot(recorded: &Path, name: &str, test: &str) {
    let expected_path = snapshot_path(name);
    if std::env::var_os("UPDATE_VIDEO_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(expected_path.parent().unwrap()).unwrap();
        std::fs::copy(recorded, &expected_path).unwrap();
        return;
    }
    let actual = decode_mp4(recorded, 320);
    let expected = decode_mp4(&expected_path, 320);
    let difference = video_difference(&actual, &expected);
    let quality = (100.0 - difference * 100.0 / 255.0).max(0.0);
    let passed = difference < 20.0;
    eprintln!(
        "VIDEO_METRIC test={test} snapshot={name} difference={difference:.2} quality={quality:.1} pass={passed}"
    );
    if !passed {
        VIDEO_FAILURES.with(|failures| {
            failures
                .borrow_mut()
                .push(format!("{name}: difference {difference:.1}"))
        });
    }
}

#[test]
fn three_monitors_disconnect_mp4_thirty_second_snapshots() {
    let _video_check = VideoCheck;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            mp4_source("window-a.mp4", 320, 180),
            timed_mp4_source(
                "window-b.mp4",
                400,
                300,
                Some(Duration::from_secs(5)..Duration::from_secs(12)),
                None,
            ),
            closing_mp4_source("window-c.mp4", 640, 360, Duration::from_secs(15)),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let paths = record(dir.path(), req, 30.0);
    assert_eq!(paths.len(), 3);
    assert_mp4_snapshot(
        &paths[0],
        "monitor-disconnect-1.mp4",
        "three_monitors_disconnect",
    );
    assert_mp4_snapshot(
        &paths[1],
        "monitor-disconnect-2-restored.mp4",
        "three_monitors_disconnect",
    );
    assert_mp4_snapshot(
        &paths[2],
        "monitor-disconnect-3-offline.mp4",
        "three_monitors_disconnect",
    );
    write_comparative_doc(
        "three-monitors-disconnect",
        &[
            DocColumn {
                name: "Монитор 1 (320×180)",
                expected: snapshot_path("monitor-disconnect-1.mp4"),
                actual: paths[0].clone(),
                state: vec![span(0.0, 30.0, "ПОДКЛЮЧЁН")],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
            DocColumn {
                name: "Монитор 2 (400×300)",
                expected: snapshot_path("monitor-disconnect-2-restored.mp4"),
                actual: paths[1].clone(),
                state: vec![
                    span(0.0, 5.0, "ПОДКЛЮЧЁН"),
                    span(5.0, 12.0, "ОТКЛЮЧЁН"),
                    span(12.0, 30.0, "ПОДКЛЮЧЁН"),
                ],
                expectation: vec![
                    span(0.0, 5.0, "Ожидается: видео"),
                    span(5.0, 12.0, "Ожидается: заглушка"),
                    span(12.0, 30.0, "Ожидается: видео"),
                ],
            },
            DocColumn {
                name: "Монитор 3 (640×360)",
                expected: snapshot_path("monitor-disconnect-3-offline.mp4"),
                actual: paths[2].clone(),
                state: vec![span(0.0, 15.0, "ПОДКЛЮЧЁН"), span(15.0, 30.0, "ОТКЛЮЧЁН")],
                expectation: vec![
                    span(0.0, 15.0, "Ожидается: видео"),
                    span(15.0, 30.0, "Ожидается: заглушка"),
                ],
            },
        ],
    );
}

#[test]
fn two_windows_close_with_fake_off_mp4_thirty_second_snapshot() {
    let _video_check = VideoCheck;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            mp4_source("window-a.mp4", 320, 180),
            closing_mp4_source("window-b.mp4", 480, 320, Duration::from_secs(8)),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    )
    .no_signal(false);
    let paths = record(dir.path(), req, 30.0);
    assert_eq!(paths.len(), 2);
    assert_mp4_snapshot(
        &paths[0],
        "close-fake-off-live.mp4",
        "two_windows_close_fake_off",
    );
    assert_mp4_snapshot(
        &paths[1],
        "close-fake-off-last-frame.mp4",
        "two_windows_close_fake_off",
    );
    write_comparative_doc(
        "two-windows-close-fake-off",
        &[
            DocColumn {
                name: "Окно 1 (320×180)",
                expected: snapshot_path("close-fake-off-live.mp4"),
                actual: paths[0].clone(),
                state: vec![span(0.0, 30.0, "ОТКРЫТО")],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
            DocColumn {
                name: "Окно 2 (480×320)",
                expected: snapshot_path("close-fake-off-last-frame.mp4"),
                actual: paths[1].clone(),
                state: vec![span(0.0, 8.0, "ОТКРЫТО"), span(8.0, 30.0, "ЗАКРЫТО")],
                expectation: vec![
                    span(0.0, 8.0, "Ожидается: видео"),
                    span(8.0, 30.0, "Ожидается: последний кадр"),
                ],
            },
        ],
    );
}

#[test]
fn deferred_window_mp4_starts_with_saved_size_and_resumes() {
    let _video_check = VideoCheck;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            deferred_pattern(400, 300, Duration::from_secs(5)),
            pattern(320, 180),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let paths = record(dir.path(), req, 30.0);
    assert_mp4_snapshot(&paths[0], "deferred-start-400x300.mp4", "deferred_window");
    assert_mp4_snapshot(&paths[1], "deferred-live-320x180.mp4", "deferred_window");
    write_comparative_doc(
        "deferred-window",
        &[
            DocColumn {
                name: "Окно 1 (400×300)",
                expected: snapshot_path("deferred-start-400x300.mp4"),
                actual: paths[0].clone(),
                state: vec![span(0.0, 5.0, "СВЁРНУТО"), span(5.0, 30.0, "ОТКРЫТО")],
                expectation: vec![
                    span(0.0, 5.0, "Ожидается: заглушка"),
                    span(5.0, 30.0, "Ожидается: видео"),
                ],
            },
            DocColumn {
                name: "Окно 2 (320×180)",
                expected: snapshot_path("deferred-live-320x180.mp4"),
                actual: paths[1].clone(),
                state: vec![span(0.0, 30.0, "ОТКРЫТО")],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
        ],
    );
}

#[test]
fn three_monitors_mp4_thirty_second_snapshots() {
    let _video_check = VideoCheck;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            mp4_source("window-a.mp4", 320, 180),
            mp4_source("window-b.mp4", 400, 300),
            mp4_source("window-c.mp4", 640, 360),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let paths = record(dir.path(), req, 30.0);
    assert_eq!(paths.len(), 3);
    assert_mp4_snapshot(&paths[0], "thirty-monitor-1.mp4", "three_monitors");
    assert_mp4_snapshot(&paths[1], "thirty-monitor-2.mp4", "three_monitors");
    assert_mp4_snapshot(&paths[2], "thirty-monitor-3.mp4", "three_monitors");
    write_comparative_doc(
        "three-monitors",
        &[
            DocColumn {
                name: "Монитор 1 (320×180)",
                expected: snapshot_path("thirty-monitor-1.mp4"),
                actual: paths[0].clone(),
                state: vec![span(0.0, 30.0, "ПОДКЛЮЧЁН")],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
            DocColumn {
                name: "Монитор 2 (400×300)",
                expected: snapshot_path("thirty-monitor-2.mp4"),
                actual: paths[1].clone(),
                state: vec![span(0.0, 30.0, "ПОДКЛЮЧЁН")],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
            DocColumn {
                name: "Монитор 3 (640×360)",
                expected: snapshot_path("thirty-monitor-3.mp4"),
                actual: paths[2].clone(),
                state: vec![span(0.0, 30.0, "ПОДКЛЮЧЁН")],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
        ],
    );
}

#[test]
fn three_windows_mp4_thirty_second_snapshots() {
    let _video_check = VideoCheck;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            mp4_source("window-a.mp4", 320, 180),
            timed_mp4_source(
                "window-b.mp4",
                400,
                300,
                Some(Duration::from_secs(5)..Duration::from_secs(12)),
                None,
            ),
            timed_mp4_source(
                "window-c.mp4",
                360,
                640,
                None,
                Some((Duration::from_secs(15), 600, 800)),
            ),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let paths = record(dir.path(), req, 30.0);
    assert_eq!(paths.len(), 3);
    for path in &paths {
        assert!(
            probe(path)
                .unwrap()
                .duration_secs()
                .is_some_and(|duration| duration > 25.0),
            "{}",
            path.display()
        );
    }
    assert_mp4_snapshot(&paths[0], "thirty-window-1.mp4", "three_windows");
    assert_mp4_snapshot(&paths[1], "thirty-window-minimized.mp4", "three_windows");
    assert_mp4_snapshot(&paths[2], "thirty-window-resized.mp4", "three_windows");
    write_comparative_doc(
        "three-windows",
        &[
            DocColumn {
                name: "Окно 1 (320×180)",
                expected: snapshot_path("thirty-window-1.mp4"),
                actual: paths[0].clone(),
                state: vec![span(0.0, 30.0, "ОТКРЫТО")],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
            DocColumn {
                name: "Окно 2 (400×300)",
                expected: snapshot_path("thirty-window-minimized.mp4"),
                actual: paths[1].clone(),
                state: vec![
                    span(0.0, 5.0, "ОТКРЫТО"),
                    span(5.0, 12.0, "СВЁРНУТО"),
                    span(12.0, 30.0, "ОТКРЫТО"),
                ],
                expectation: vec![
                    span(0.0, 5.0, "Ожидается: видео"),
                    span(5.0, 12.0, "Ожидается: заглушка"),
                    span(12.0, 30.0, "Ожидается: видео"),
                ],
            },
            DocColumn {
                name: "Окно 3 (360×640)",
                expected: snapshot_path("thirty-window-resized.mp4"),
                actual: paths[2].clone(),
                state: vec![
                    span(0.0, 15.0, "ОТКРЫТО"),
                    span(15.0, 30.0, "ИЗМЕНЁН РАЗМЕР"),
                ],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
        ],
    );
}

#[test]
fn single_window_fake_on_mp4_thirty_second_snapshot() {
    let _video_check = VideoCheck;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![timed_mp4_source(
            "window-a.mp4",
            320,
            180,
            Some(Duration::from_secs(5)..Duration::from_secs(12)),
            None,
        )],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let paths = record(dir.path(), req, 30.0);
    assert_mp4_snapshot(
        &paths[0],
        "single-window-fake-on.mp4",
        "single_window_fake_on",
    );
    write_comparative_doc(
        "single-window-fake-on",
        &[DocColumn {
            name: "Окно",
            expected: snapshot_path("single-window-fake-on.mp4"),
            actual: paths[0].clone(),
            state: vec![
                span(0.0, 5.0, "ОТКРЫТО"),
                span(5.0, 12.0, "СВЁРНУТО"),
                span(12.0, 30.0, "ОТКРЫТО"),
            ],
            expectation: vec![
                span(0.0, 5.0, "Ожидается: видео"),
                span(5.0, 12.0, "Ожидается: заглушка"),
                span(12.0, 30.0, "Ожидается: видео"),
            ],
        }],
    );
}

#[test]
fn single_window_fake_off_mp4_thirty_second_snapshot() {
    let _video_check = VideoCheck;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![timed_mp4_source(
            "window-a.mp4",
            320,
            180,
            Some(Duration::from_secs(5)..Duration::from_secs(12)),
            None,
        )],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    )
    .no_signal(false);
    let paths = record(dir.path(), req, 30.0);
    assert_mp4_snapshot(
        &paths[0],
        "single-window-fake-off.mp4",
        "single_window_fake_off",
    );
    write_comparative_doc(
        "single-window-fake-off",
        &[DocColumn {
            name: "Окно",
            expected: snapshot_path("single-window-fake-off.mp4"),
            actual: paths[0].clone(),
            state: vec![
                span(0.0, 5.0, "ОТКРЫТО"),
                span(5.0, 12.0, "СВЁРНУТО"),
                span(12.0, 30.0, "ОТКРЫТО"),
            ],
            expectation: vec![
                span(0.0, 5.0, "Ожидается: видео"),
                span(5.0, 12.0, "Ожидается: последний кадр"),
                span(12.0, 30.0, "Ожидается: видео"),
            ],
        }],
    );
}

#[test]
fn closed_window_mp4_thirty_second_snapshot() {
    let _video_check = VideoCheck;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            mp4_source("window-a.mp4", 320, 180),
            closing_mp4_source("window-b.mp4", 480, 320, Duration::from_secs(8)),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let paths = record(dir.path(), req, 30.0);
    assert_eq!(paths.len(), 2);
    assert_mp4_snapshot(
        &paths[0],
        "closed-two-windows-live.mp4",
        "closed_two_windows",
    );
    assert_mp4_snapshot(
        &paths[1],
        "closed-two-windows-closed.mp4",
        "closed_two_windows",
    );
    write_comparative_doc(
        "closed-two-windows",
        &[
            DocColumn {
                name: "Окно 1 (320×180)",
                expected: snapshot_path("closed-two-windows-live.mp4"),
                actual: paths[0].clone(),
                state: vec![span(0.0, 30.0, "ОТКРЫТО")],
                expectation: vec![span(0.0, 30.0, "Ожидается: видео")],
            },
            DocColumn {
                name: "Окно 2 (480×320)",
                expected: snapshot_path("closed-two-windows-closed.mp4"),
                actual: paths[1].clone(),
                state: vec![span(0.0, 8.0, "ОТКРЫТО"), span(8.0, 30.0, "ЗАКРЫТО")],
                expectation: vec![
                    span(0.0, 8.0, "Ожидается: видео"),
                    span(8.0, 30.0, "Ожидается: заглушка"),
                ],
            },
        ],
    );
}
