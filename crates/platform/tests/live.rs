use mediaa_core::source::{FrameSource, SourceFactory};
use mediaa_core::{OutputKind, Rect, VideoContainer};
use mediaa_media::recorder::{RecordingRequest, RecordingSession, SessionEvent};
use mediaa_platform::screen::{self, MonitorSource};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

#[test]
#[ignore = "needs a desktop session"]
fn captures_primary_monitor() {
    let monitors = screen::monitors().unwrap();
    assert!(!monitors.is_empty());
    let m = &monitors[0];
    assert!(m.primary || monitors.len() == 1);
    let f = screen::capture_monitor(m.id).unwrap();
    assert_eq!((f.width, f.height), (m.width, m.height));
    assert!(
        f.data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| *p != [0, 0, 0, 255]),
        "not a black frame"
    );
}

#[test]
#[ignore = "needs a desktop session"]
fn region_source_has_even_clamped_size() {
    let m = screen::monitors().unwrap().remove(0);
    let mut src = MonitorSource::new(m.id, Some(Rect::new(11, 13, 301, 201)), true).unwrap();
    assert_eq!(src.size(), (300, 200));
    let f = src.grab().unwrap();
    assert_eq!((f.width, f.height), (300, 200));
}

#[test]
#[ignore = "needs a desktop session"]
fn lists_and_captures_a_window() {
    let windows = screen::windows().unwrap();
    let Some(w) = windows.first() else { return };
    let f = screen::capture_window(w.id).unwrap();
    assert!(f.width > 0 && f.height > 0);
}

#[test]
#[ignore = "needs a desktop session"]
fn records_real_screen_area_to_mp4() {
    let dir = tempfile::tempdir().unwrap();
    let m = screen::monitors().unwrap().remove(0);
    let id = m.id;
    let factory: SourceFactory = Box::new(move || {
        MonitorSource::new(id, Some(Rect::new(0, 0, 640, 360)), true)
            .map(|s| Box::new(s) as Box<dyn FrameSource>)
    });
    let session = RecordingSession::start(RecordingRequest {
        sources: vec![factory],
        audio: vec![],
        output: OutputKind::Video(VideoContainer::Mp4),
        fps: 15,
        video_quality: 60,
        max_height: None,
        audio_in_every_file: true,
        audio_kbps: 192,
        anim: Default::default(),
        segmented_webp: true,
        parallel_webp: true,
        out_dir: dir.path().join("out"),
        work_dir: dir.path().join("work"),
        title: "live".into(),
    })
    .unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let events = session.events();
    session
        .stop(true, Arc::new(AtomicBool::new(false)))
        .join()
        .unwrap();
    let paths = loop {
        match events.recv_timeout(Duration::from_secs(30)).unwrap() {
            SessionEvent::Finished(p) => break p,
            SessionEvent::Finalizing(_) | SessionEvent::Warning(_) | SessionEvent::Notice(_) => {}
            other => panic!("{other:?}"),
        }
    };
    let info = mediaa_media::info::probe(&paths[0]).unwrap();
    assert_eq!((info.width, info.height), (640, 360));
    let d = info.duration_secs().unwrap();
    assert!((1.0..2.2).contains(&d), "duration {d}");
}
