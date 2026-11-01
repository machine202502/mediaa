use super::{
    cleanup_old_selected, cleanup_selected, plan_cleanup, CleanupItem, CleanupOlder, CleanupPlan,
    LibraryController,
};
use crate::wake::Wake;
use mediaa_core::library::LibraryEntry;
use mediaa_storage::library_index::LibraryIndex;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[test]
fn selection_clicks_and_day_toggle() {
    let mut controller = LibraryController::default();
    let a = std::path::PathBuf::from("a.png");
    let b = std::path::PathBuf::from("b.png");
    controller.select_click(&a, false, false);
    assert_eq!(controller.selected, vec![a.clone()]);
    controller.select_click(&b, true, false);
    assert_eq!(controller.selected, vec![a.clone(), b.clone()]);
    controller.select_click(&a, true, false);
    assert_eq!(controller.selected, vec![b.clone()]);

    let ea = entry(a.clone(), SystemTime::UNIX_EPOCH);
    let eb = entry(b.clone(), SystemTime::UNIX_EPOCH);
    controller.select_day(&[&ea, &eb]);
    assert_eq!(controller.selected.len(), 2);
    controller.select_day(&[&ea, &eb]);
    assert!(controller.selected.is_empty());

    controller.select_click(&a, false, false);
    controller.clear_selection();
    assert!(controller.selected.is_empty());
}

#[test]
fn bulk_delete_moves_every_file_to_trash() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.png");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();
    let mut controller = LibraryController::default();
    controller.set_test_trash();
    controller.delete_many(vec![a.clone(), b.clone()]).unwrap();
    let report = (0..100)
        .find_map(|_| {
            let report = controller.poll_cleanup(&Wake::default());
            if report.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
            report
        })
        .expect("removal should finish")
        .0;
    assert_eq!(report.removed.len(), 2);
    assert!(!a.exists());
    assert!(!b.exists());
}

#[test]
fn a_running_scan_is_not_duplicated_by_manual_refresh() {
    use crate::logic::jobs::Job;
    use mediaa_core::Settings;
    let dir = tempfile::tempdir().unwrap();
    let scan_job = Job::spawn("library-test-scan", Arc::default(), Vec::new).unwrap();
    let mut ui = LibraryController {
        index: Some(LibraryIndex::load(dir.path())),
        scan_job: Some(scan_job),
        ..LibraryController::default()
    };
    let settings = Settings {
        media_dir: dir.path().to_path_buf(),
        ..Settings::default()
    };
    ui.refresh(&settings);
    assert!(ui.scan_job.is_some());
}

fn entry(path: std::path::PathBuf, modified: SystemTime) -> LibraryEntry {
    LibraryEntry {
        name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        title: "Recording".into(),
        path,
        kind: mediaa_core::MediaKind::Video,
        size: 1024,
        modified,
        taken: chrono::NaiveDateTime::default(),
        details: None,
    }
}

#[test]
fn unchanged_library_views_reuse_the_same_entry_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Recording_2026-01-01_10-00-00.mkv");
    let mut index = LibraryIndex::load(dir.path());
    index.apply_scan(vec![entry(path.clone(), SystemTime::UNIX_EPOCH)]);
    let mut library = LibraryController {
        index: Some(index),
        ..LibraryController::default()
    };
    let first = library.hits().unwrap();
    let second = library.hits().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    library.index.as_mut().unwrap().set_details(
        &path,
        mediaa_core::library::Details {
            width: 640,
            height: 360,
            ..Default::default()
        },
    );
    let detailed = library.hits().unwrap();
    assert!(!Arc::ptr_eq(&second, &detailed));
    assert_eq!(detailed.entries[0].details.unwrap().width, 640);
    library.limit += super::CHUNK;
    assert!(!Arc::ptr_eq(&detailed, &library.hits().unwrap()));
}

#[test]
fn cleanup_thresholds_use_strict_seven_and_thirty_day_boundaries() {
    let now = SystemTime::now();
    let days = |n: u64| {
        now.checked_sub(Duration::from_secs(n * 24 * 60 * 60))
            .unwrap()
    };
    let entries = [
        entry(
            "fresh.png".into(),
            now.checked_sub(Duration::from_secs(6 * 24 * 60 * 60))
                .unwrap(),
        ),
        entry("week.png".into(), days(7)),
        entry("older-week.png".into(), days(8)),
        entry("month.png".into(), days(30)),
        entry("older-month.png".into(), days(31)),
    ];
    let week = plan_cleanup(&entries, CleanupOlder::Week, now);
    let month = plan_cleanup(&entries, CleanupOlder::Month, now);
    assert_eq!(
        week.items
            .iter()
            .map(|i| i.path.as_path())
            .collect::<Vec<_>>(),
        ["older-week.png", "month.png", "older-month.png"]
    );
    assert_eq!(
        month
            .items
            .iter()
            .map(|i| i.path.as_path())
            .collect::<Vec<_>>(),
        ["older-month.png"]
    );
}

#[test]
fn cleanup_rechecks_the_file_age_before_moving_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("old.png");
    std::fs::write(&path, b"new content").unwrap();
    let item = CleanupItem {
        path: path.clone(),
        size: 1,
        modified: None,
    };
    let mut calls = 0;
    let report = cleanup_old_selected(
        std::slice::from_ref(&item),
        CleanupOlder::Week,
        SystemTime::now(),
        |_| {
            calls += 1;
            Ok(())
        },
    );
    assert_eq!(calls, 0);
    assert_eq!(report.skipped, 1);

    let old = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
    std::fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let report = cleanup_old_selected(&[item], CleanupOlder::Week, SystemTime::now(), |_| {
        calls += 1;
        Ok(())
    });
    assert_eq!(calls, 1);
    assert_eq!(report.removed.len(), 1);
    assert_eq!(report.removed[0].size, 11);
}

#[test]
fn bulk_cleanup_reports_back_without_blocking() {
    let mut library = LibraryController::default();
    let plan = CleanupPlan {
        older: CleanupOlder::Week,
        items: Vec::new(),
    };
    library.start_cleanup(&plan).unwrap();
    assert!(library.cleanup_active());
    let result = (0..100)
        .find_map(|_| {
            let result = library.poll_cleanup(&Wake::default());
            if result.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
            result
        })
        .expect("cleanup should finish");
    assert!(result.0.removed.is_empty());
    assert!(!library.cleanup_active());
}

#[test]
fn cleanup_continues_after_a_failed_move_and_removes_ready_proxies() {
    let dir = tempfile::tempdir().unwrap();
    let items: Vec<CleanupItem> = ["first.webp", "busy.webp", "last.webp"]
        .into_iter()
        .map(|name| CleanupItem {
            path: dir.path().join(name),
            size: 10,
            modified: None,
        })
        .collect();
    for item in &items {
        std::fs::write(&item.path, b"media").unwrap();
        let proxy = mediaa_media::preview_cache::proxy_path(&item.path);
        let hover = mediaa_media::preview_cache::hover_proxy_path(&item.path);
        std::fs::create_dir_all(proxy.parent().unwrap()).unwrap();
        std::fs::write(proxy, b"proxy").unwrap();
        std::fs::write(hover, b"hover").unwrap();
    }
    let mut trashed = Vec::new();
    let report = cleanup_selected(&items, |path| {
        trashed.push(path.to_path_buf());
        if path.file_name().is_some_and(|name| name == "busy.webp") {
            anyhow::bail!("file is busy")
        }
        std::fs::remove_file(path)?;
        Ok(())
    });
    assert_eq!(report.removed.len(), 2);
    assert!(trashed
        .iter()
        .all(|path| items.iter().any(|item| &item.path == path)));
    assert_eq!(report.failures.len(), 1);
    assert!(!items[0].path.exists());
    assert!(items[1].path.exists());
    assert!(!items[2].path.exists());
    assert!(!mediaa_media::preview_cache::proxy_path(&items[0].path).exists());
    assert!(!mediaa_media::preview_cache::hover_proxy_path(&items[0].path).exists());
    assert!(mediaa_media::preview_cache::proxy_path(&items[1].path).exists());
    assert!(mediaa_media::preview_cache::hover_proxy_path(&items[1].path).exists());
    assert!(!mediaa_media::preview_cache::proxy_path(&items[2].path).exists());
    assert!(!mediaa_media::preview_cache::hover_proxy_path(&items[2].path).exists());
}

#[test]
fn cleanup_removes_the_library_index_rows() {
    let dir = tempfile::tempdir().unwrap();
    let media = dir.path().join("Old_2026-01-01_00-00-00.webp");
    std::fs::write(&media, b"media").unwrap();
    let old = SystemTime::now()
        .checked_sub(Duration::from_secs(31 * 24 * 60 * 60))
        .unwrap();
    std::fs::File::options()
        .write(true)
        .open(&media)
        .unwrap()
        .set_modified(old)
        .unwrap();
    let mut index = LibraryIndex::load(dir.path());
    index.apply_scan(mediaa_storage::library_index::scan_dir(dir.path()));
    assert_eq!(index.len(), 1);
    let plan = plan_cleanup(index.entries(), CleanupOlder::Month, SystemTime::now());
    let report = cleanup_selected(&plan.items, |path| {
        std::fs::remove_file(path)?;
        Ok(())
    });
    let removed = report
        .removed
        .iter()
        .map(|item| item.path.clone())
        .collect::<Vec<_>>();
    index.refresh_paths(&removed);
    assert!(index.is_empty());
    assert!(LibraryIndex::load(dir.path()).is_empty());
}

#[test]
fn a_file_that_is_already_gone_is_not_a_delete_failure() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a_2026-01-01_10-00-00.png");
    let b = dir.path().join("b_2026-01-01_10-00-00.png");
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();
    let mut controller = LibraryController::default();
    controller.set_test_entries(dir.path());
    std::fs::remove_file(&a).unwrap();
    std::fs::remove_file(&b).unwrap();
    controller.delete_many(vec![a, b]).unwrap();
    for _ in 0..200 {
        if let Some((report, _)) = controller.poll_cleanup(&Wake::default()) {
            assert!(report.failures.is_empty(), "{:?}", report.failures);
            assert_eq!(report.removed.len(), 2);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("removal did not finish");
}

#[test]
fn renaming_updates_selected_paths_and_the_anchor() {
    let old = std::path::PathBuf::from("old.png");
    let new = std::path::PathBuf::from("new.png");
    let mut controller = LibraryController::default();
    controller.select_for_test(vec![old.clone()]);
    controller.rename_selected(&old, &new);
    assert_eq!(controller.selected, vec![new.clone()]);
    assert_eq!(controller.anchor, Some(new));
}
#[test]
fn deleting_removes_deleted_paths_from_the_selection() {
    let a = std::path::PathBuf::from("a.png");
    let b = std::path::PathBuf::from("b.png");
    let mut controller = LibraryController::default();
    controller.select_for_test(vec![a.clone(), b.clone()]);
    controller.remove_selected(&[a]);
    assert_eq!(controller.selected, vec![b.clone()]);
    assert_eq!(controller.anchor, Some(b));
    controller.remove_selected(&[std::path::PathBuf::from("b.png")]);
    assert!(controller.selected.is_empty());
    assert_eq!(controller.anchor, None);
}
