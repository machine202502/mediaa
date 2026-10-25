use super::{
    cleanup_modal, cleanup_old_selected, cleanup_plan, cleanup_selected, cover_uv, grid_columns,
    tools, CleanupItem, CleanupOlder, CleanupPlan, LibraryUi, GAP, MAIN_TOOLS, MIN_TILE,
};
use eframe::egui::vec2;
use mediaa_core::library::LibraryEntry;
use mediaa_storage::library_index::LibraryIndex;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

#[test]
fn a_running_scan_is_not_duplicated_by_manual_refresh() {
    use crate::app::tasks::TaskSupervisor;
    use mediaa_core::Settings;
    let dir = tempfile::tempdir().unwrap();
    let (_sender, receiver) = crossbeam_channel::bounded(1);
    let mut ui = LibraryUi {
        index: Some(LibraryIndex::load(dir.path())),
        scan: Some(receiver),
        ..LibraryUi::default()
    };
    let settings = Settings {
        media_dir: dir.path().to_path_buf(),
        ..Settings::default()
    };
    ui.refresh(&settings, &mut TaskSupervisor::default());
    assert!(ui.scan.is_some());
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
fn unchanged_library_views_reuse_the_same_entry_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Recording_2026-01-01_10-00-00.mkv");
    let mut index = LibraryIndex::load(dir.path());
    index.apply_scan(vec![entry(path.clone(), SystemTime::UNIX_EPOCH)]);
    let mut ui = LibraryUi {
        index: Some(index),
        ..LibraryUi::default()
    };
    let first = ui.hits().unwrap();
    let second = ui.hits().unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    ui.index.as_mut().unwrap().set_details(
        &path,
        mediaa_core::library::Details {
            width: 640,
            height: 360,
            ..Default::default()
        },
    );
    let detailed = ui.hits().unwrap();
    assert!(!Arc::ptr_eq(&second, &detailed));
    assert_eq!(detailed.entries[0].details.unwrap().width, 640);
    ui.limit += super::CHUNK;
    assert!(!Arc::ptr_eq(&detailed, &ui.hits().unwrap()));
}

#[test]
fn more_button_opens_the_menu_and_its_items_act() {
    use super::{tools_strip, Action};
    use egui_kittest::kittest::Queryable;
    use mediaa_core::library::LibraryEntry;
    let e = LibraryEntry {
        path: "/m/A_2026-01-01_10-00-00.png".into(),
        name: "A_2026-01-01_10-00-00.png".into(),
        title: "A".into(),
        kind: mediaa_core::MediaKind::Image,
        size: 1,
        modified: std::time::SystemTime::UNIX_EPOCH,
        taken: chrono::NaiveDateTime::default(),
        details: None,
    };
    let list = tools(&e);
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui, chosen: &mut Option<Action>| {
            ui.set_width(300.0);
            if let Some(a) = tools_strip(ui, &list) {
                *chosen = Some(a);
            }
        },
        None,
    );
    let rename = rust_i18n::t!("lib.rename").to_string();
    assert!(
        harness.query_by_label(&rename).is_none(),
        "menu closed at first"
    );
    harness.get_by_label(&rust_i18n::t!("lib.more")).click();
    harness.run();
    assert!(
        harness.query_by_label(&rename).is_some(),
        "⋯ opens the menu"
    );
    harness.get_by_label(&rename).click();
    harness.run();
    assert!(
        matches!(harness.state(), Some(Action::AskRename(_, t)) if t == "A"),
        "the item's action is returned"
    );
    assert!(
        harness.query_by_label(&rename).is_none(),
        "menu closes after a choice"
    );
    harness.get_by_label(&rust_i18n::t!("lib.reveal")).click();
    harness.run();
    assert!(matches!(harness.state(), Some(Action::Reveal(_))));
}

#[test]
fn every_kind_gets_all_tools_and_few_main_ones() {
    use mediaa_core::library::LibraryEntry;
    use mediaa_core::MediaKind;
    for kind in [
        MediaKind::Image,
        MediaKind::Video,
        MediaKind::Animation,
        MediaKind::Audio,
    ] {
        let e = LibraryEntry {
            path: "/m/A_2026-01-01_10-00-00.png".into(),
            name: "A_2026-01-01_10-00-00.png".into(),
            title: "A".into(),
            kind,
            size: 1,
            modified: std::time::SystemTime::UNIX_EPOCH,
            taken: chrono::NaiveDateTime::default(),
            details: None,
        };
        let list = tools(&e);
        assert!(
            list.iter().filter(|t| t.main).count() <= MAIN_TOOLS,
            "{kind:?}: main tools fit the tile"
        );
        assert!(
            list.last().is_some_and(|t| t.danger) && list.iter().filter(|t| t.danger).count() == 1,
            "delete last"
        );
        assert_eq!(
            list.len(),
            if kind == MediaKind::Image { 9 } else { 8 },
            "{kind:?}"
        );
    }
}

#[test]
fn grid_rows_always_fit_the_window() {
    for avail in [
        120.0_f32,
        250.0,
        MIN_TILE,
        MIN_TILE + 1.0,
        600.0,
        1000.0,
        1333.0,
        1999.0,
        3000.0,
    ] {
        let (cols, w) = grid_columns(avail);
        let row = cols as f32 * w + GAP * (cols - 1) as f32;
        assert!(row <= avail.max(1.0), "{avail}: {cols} × {w} = {row}");
        assert!(
            cols == 1 || w >= MIN_TILE,
            "{avail}: tiles never narrower than the minimum"
        );
        assert!(
            avail - row < cols as f32 + 1.0,
            "{avail}: row filled ({row})"
        );
    }
    assert_eq!(grid_columns(1000.0).0, 4);
}

#[test]
fn cover_crops_the_longer_side_symmetrically() {
    let wide = cover_uv(vec2(200.0, 100.0), vec2(100.0, 100.0));
    assert_eq!(
        (wide.min.x, wide.max.x, wide.min.y, wide.max.y),
        (0.25, 0.75, 0.0, 1.0)
    );
    let tall = cover_uv(vec2(100.0, 200.0), vec2(100.0, 100.0));
    assert_eq!(
        (tall.min.y, tall.max.y, tall.min.x, tall.max.x),
        (0.25, 0.75, 0.0, 1.0)
    );
    let same = cover_uv(vec2(160.0, 90.0), vec2(320.0, 180.0));
    assert_eq!((same.min.x, same.max.x), (0.0, 1.0));
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
    let week = cleanup_plan(&entries, CleanupOlder::Week, now);
    let month = cleanup_plan(&entries, CleanupOlder::Month, now);
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
fn bulk_cleanup_reports_back_without_blocking_the_library_ui() {
    let ctx = eframe::egui::Context::default();
    let mut library = LibraryUi::default();
    let plan = CleanupPlan {
        older: CleanupOlder::Week,
        items: Vec::new(),
    };
    let mut tasks = crate::app::tasks::TaskSupervisor::default();
    library.start_cleanup(&plan, &mut tasks).unwrap();
    assert!(library.cleanup_active());
    let result = (0..100)
        .find_map(|_| {
            let result = library.poll_cleanup(&mut tasks, &ctx);
            if result.is_none() {
                std::thread::sleep(Duration::from_millis(5));
            }
            result
        })
        .expect("cleanup should finish");
    assert!(result.1.removed.is_empty());
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
        })
        .collect();
    for item in &items {
        std::fs::write(&item.path, b"media").unwrap();
        let proxy = mediaa_media::preview_cache::proxy_path(&item.path);
        std::fs::create_dir_all(proxy.parent().unwrap()).unwrap();
        std::fs::write(proxy, b"proxy").unwrap();
    }
    let report = cleanup_selected(&items, |path| {
        if path.file_name().is_some_and(|name| name == "busy.webp") {
            anyhow::bail!("file is busy")
        }
        std::fs::remove_file(path)?;
        Ok(())
    });
    assert_eq!(report.removed.len(), 2);
    assert_eq!(report.failures.len(), 1);
    assert!(!items[0].path.exists());
    assert!(items[1].path.exists());
    assert!(!items[2].path.exists());
    assert!(!mediaa_media::preview_cache::proxy_path(&items[0].path).exists());
    assert!(mediaa_media::preview_cache::proxy_path(&items[1].path).exists());
    assert!(!mediaa_media::preview_cache::proxy_path(&items[2].path).exists());
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
    let plan = cleanup_plan(index.entries(), CleanupOlder::Month, SystemTime::now());
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
fn cleanup_modal_shows_the_scope_and_can_be_cancelled() {
    use egui_kittest::kittest::Queryable;
    if let Ok(locale) = std::env::var("MEDIAA_LIBRARY_MODAL_LOCALE") {
        rust_i18n::set_locale(&locale);
    }
    let plan = cleanup_plan(
        &[entry(
            "old.webp".into(),
            SystemTime::now()
                .checked_sub(Duration::from_secs(31 * 24 * 60 * 60))
                .unwrap(),
        )],
        CleanupOlder::Month,
        SystemTime::now(),
    );
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut eframe::egui::Ui, decision: &mut Option<bool>| {
            if let Some(value) = cleanup_modal(ui.ctx(), &plan) {
                *decision = Some(value);
            }
        },
        None,
    );
    crate::theme::install(&harness.ctx);
    harness.run();
    assert!(harness
        .query_by_label(&rust_i18n::t!("lib.cleanup_month"))
        .is_some());
    assert!(harness
        .query_by_label(&rust_i18n::t!(
            "lib.cleanup_text",
            count = 1,
            size = "1.0 KB"
        ))
        .is_some());
    if let Ok(path) = std::env::var("MEDIAA_LIBRARY_MODAL_SNAPSHOT") {
        harness.render().unwrap().save(path).unwrap();
    }
    harness
        .get_by_label_contains(&rust_i18n::t!("common.cancel"))
        .click();
    harness.run();
    assert_eq!(harness.state(), &Some(false));
}
