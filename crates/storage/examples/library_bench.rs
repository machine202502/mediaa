use image::codecs::jpeg::JpegEncoder;
use mediaa_core::Frame;
use mediaa_storage::library_index::{scan_dir, LibraryIndex, LibraryWatcher};
use mediaa_storage::thumb_cache::ThumbCache;
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("generate") => generate(&args[2], args[3].parse()?),
        Some("scan") => scan(&args[2], args.get(3).map_or(Ok(5), |v| v.parse())?),
        Some("watch") => watch(&args[2], args[3].parse()?),
        Some("rescan") => rescan(&args[2], args.get(3).map_or(Ok(50), |v| v.parse())?),
        Some("atlas") => atlas(&args[2], args[3].parse()?),
        Some("thumbs") => thumbs(&args[2], args[3].parse()?),
        _ => anyhow::bail!(
            "usage: library_bench generate <dir> <count> | scan <dir> [runs] | watch <dir> <count> | rescan <dir> [changes] | atlas <dir> <count> | thumbs <dir> <count>"
        ),
    }
}

fn generate(path: &str, count: usize) -> anyhow::Result<()> {
    let dir = Path::new(path);
    std::fs::create_dir_all(dir)?;
    for index in 0..count {
        create_media_file(dir, index)?;
    }
    println!("generated={count} dir={}", dir.display());
    Ok(())
}

fn watch(path: &str, count: usize) -> anyhow::Result<()> {
    let dir = Path::new(path);
    std::fs::create_dir_all(dir)?;
    let watcher = LibraryWatcher::new(dir, || {})?;
    let mut seen = std::collections::HashSet::new();
    let mut rescan = false;
    for index in 0..count {
        create_media_file(dir, index)?;
        if index % 10 == 0 {
            collect_changes(&watcher, &mut seen, &mut rescan);
        }
    }
    std::thread::sleep(Duration::from_secs(2));
    collect_changes(&watcher, &mut seen, &mut rescan);
    let listing = scan_dir(dir);
    let missing = listing
        .iter()
        .filter(|entry| !seen.contains(&entry.name))
        .count();
    println!(
        "entries={} changed_paths={} rescan={} missed_paths={}",
        listing.len(),
        seen.len(),
        rescan,
        missing
    );
    Ok(())
}

fn rescan(path: &str, changes: usize) -> anyhow::Result<()> {
    let dir = Path::new(path);
    let mut listing = scan_dir(dir);
    anyhow::ensure!(listing.len() > changes, "library is too small");
    let mut index = LibraryIndex::load(dir);
    index.apply_scan(listing.clone());
    let watcher = LibraryWatcher::new(dir, || {})?;

    let removed: Vec<PathBuf> = listing
        .iter()
        .take(changes)
        .map(|entry| entry.path.clone())
        .collect();
    for path in &removed {
        std::fs::remove_file(path)?;
    }
    let first_new = listing.len();
    for index in first_new..first_new + changes {
        create_media_file(dir, index)?;
    }
    std::thread::sleep(Duration::from_secs(2));

    let mut seen = std::collections::HashSet::new();
    let mut overflow = false;
    collect_changes(&watcher, &mut seen, &mut overflow);
    let observed: Vec<PathBuf> = listing
        .iter()
        .filter(|entry| {
            entry
                .path
                .file_name()
                .is_some_and(|name| seen.contains(name.to_string_lossy().as_ref()))
        })
        .map(|entry| entry.path.clone())
        .chain((first_new..first_new + changes).map(|index| media_path(dir, index)))
        .collect();

    let targeted_started = Instant::now();
    index.refresh_paths(&observed);
    let targeted = targeted_started.elapsed();

    listing = scan_dir(dir);
    let present: std::collections::HashSet<&Path> =
        listing.iter().map(|entry| entry.path.as_path()).collect();
    let stale_before_rescan = index
        .entries()
        .iter()
        .filter(|entry| !present.contains(entry.path.as_path()))
        .count();
    let indexed: std::collections::HashSet<&Path> = index
        .entries()
        .iter()
        .map(|entry| entry.path.as_path())
        .collect();
    let missing_before_rescan = listing
        .iter()
        .filter(|entry| !indexed.contains(entry.path.as_path()))
        .count();

    let scan_started = Instant::now();
    listing = scan_dir(dir);
    let scan = scan_started.elapsed();
    let apply_started = Instant::now();
    index.apply_scan(listing.clone());
    let apply = apply_started.elapsed();
    let present: std::collections::HashSet<&Path> =
        listing.iter().map(|entry| entry.path.as_path()).collect();
    let stale_after_rescan = index
        .entries()
        .iter()
        .filter(|entry| !present.contains(entry.path.as_path()))
        .count();
    let indexed: std::collections::HashSet<&Path> = index
        .entries()
        .iter()
        .map(|entry| entry.path.as_path())
        .collect();
    let missing_after_rescan = listing
        .iter()
        .filter(|entry| !indexed.contains(entry.path.as_path()))
        .count();

    println!(
        "files={} changes={} observed_paths={} overflow={overflow} targeted_refresh_ms={:.3} stale_before_rescan={stale_before_rescan} missing_before_rescan={missing_before_rescan} rescan_ms={:.3} apply_scan_ms={:.3} stale_after_rescan={stale_after_rescan} missing_after_rescan={missing_after_rescan}",
        index.entries().len(),
        changes,
        seen.len(),
        millis(targeted),
        millis(scan + apply),
        millis(apply)
    );
    Ok(())
}

fn collect_changes(
    watcher: &LibraryWatcher,
    seen: &mut std::collections::HashSet<String>,
    rescan: &mut bool,
) {
    let changes = watcher.changes();
    *rescan |= changes.rescan;
    seen.extend(changes.paths.into_iter().filter_map(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    }));
}

fn create_media_file(dir: &Path, index: usize) -> anyhow::Result<()> {
    let target = media_path(dir, index);
    File::create(target)?.write_all(b"x")?;
    Ok(())
}

fn media_path(dir: &Path, index: usize) -> PathBuf {
    let (title, extension) = match index % 4 {
        0 => ("Image", "png"),
        1 => ("Video", "mkv"),
        2 => ("Audio", "mp3"),
        _ => ("Animation", "gif"),
    };
    dir.join(format!(
        "{title}_2026-01-01_00-00-00-{index:06}.{extension}"
    ))
}

fn scan(path: &str, runs: usize) -> anyhow::Result<()> {
    let dir = Path::new(path);
    anyhow::ensure!(dir.is_dir(), "library directory does not exist");
    let _ = scan_dir(dir);
    let mut samples = Vec::new();
    let mut listing = Vec::new();
    for _ in 0..runs.max(1) {
        let started = Instant::now();
        listing = scan_dir(dir);
        samples.push(started.elapsed());
    }
    let load_started = Instant::now();
    let mut index = LibraryIndex::load(dir);
    let load = load_started.elapsed();
    let count = listing.len();
    let apply_started = Instant::now();
    index.apply_scan(listing);
    let apply = apply_started.elapsed();
    let paths: Vec<PathBuf> = index
        .entries()
        .iter()
        .take(100)
        .map(|entry| entry.path.clone())
        .collect();
    let refresh_started = Instant::now();
    index.refresh_paths(&paths);
    let refresh = refresh_started.elapsed();
    samples.sort();
    let median = percentile(&samples, 0.50);
    let p95 = percentile(&samples, 0.95);
    println!(
        "entries={count} scan_p50_ms={:.3} scan_p95_ms={:.3} scan_max_ms={:.3} index_load_ms={:.3} apply_scan_ms={:.3} refresh_100_ms={:.3}",
        millis(median),
        millis(p95),
        millis(samples.into_iter().max().unwrap_or_default()),
        millis(load),
        millis(apply),
        millis(refresh)
    );
    Ok(())
}

fn percentile(samples: &[Duration], percentile: f64) -> Duration {
    let index = ((samples.len() as f64 * percentile).ceil() as usize)
        .saturating_sub(1)
        .min(samples.len().saturating_sub(1));
    samples.get(index).copied().unwrap_or_default()
}

fn thumbs(path: &str, count: usize) -> anyhow::Result<()> {
    let root = Path::new(path);
    let atlas_dir = root.join("atlas");
    let file_dir = root.join("files");
    std::fs::create_dir_all(&atlas_dir)?;
    std::fs::create_dir_all(&file_dir)?;
    let modified = SystemTime::now();
    let names: Vec<String> = (0..count)
        .map(|index| format!("thumb-{index:06}"))
        .collect();
    let frame = test_frame();

    let mut atlas = ThumbCache::open(&atlas_dir);
    let mut atlas_write = Duration::ZERO;
    for (index, name) in names.iter().enumerate() {
        let started = Instant::now();
        anyhow::ensure!(
            atlas.put(name, index as u64, modified, &frame),
            "atlas rejected frame"
        );
        atlas_write += started.elapsed();
    }
    let atlas_flush_started = Instant::now();
    atlas.flush()?;
    let atlas_flush = atlas_flush_started.elapsed();
    drop(atlas);

    let mut files_write = Duration::ZERO;
    for name in &names {
        let image = image::RgbImage::from_raw(frame.width, frame.height, frame.data.clone())
            .ok_or_else(|| anyhow::anyhow!("invalid benchmark frame"))?;
        let target = file_dir.join(format!("{name}.jpg"));
        let started = Instant::now();
        let mut file = File::create(target)?;
        JpegEncoder::new_with_quality(&mut file, 85).encode_image(&image)?;
        files_write += started.elapsed();
    }

    let atlas_open_started = Instant::now();
    let mut atlas = ThumbCache::open(&atlas_dir);
    let atlas_open = atlas_open_started.elapsed();
    let atlas_read_started = Instant::now();
    for index in read_indices(count) {
        anyhow::ensure!(
            atlas.get(&names[index], index as u64, modified).is_some(),
            "atlas miss"
        );
    }
    let atlas_read = atlas_read_started.elapsed();

    let files_open_started = Instant::now();
    let file_handles: Vec<File> = names
        .iter()
        .map(|name| File::open(file_dir.join(format!("{name}.jpg"))))
        .collect::<Result<_, _>>()?;
    let files_open = files_open_started.elapsed();
    drop(file_handles);
    let files_read_started = Instant::now();
    for index in read_indices(count) {
        let image = image::open(file_dir.join(format!("{}.jpg", names[index])))?.to_rgba8();
        anyhow::ensure!(!image.is_empty(), "file thumbnail is empty");
    }
    let files_read = files_read_started.elapsed();

    let (atlas_files, atlas_bytes) = disk_stats(&atlas_dir)?;
    let (file_count, file_bytes) = disk_stats(&file_dir)?;
    println!(
        "count={count} atlas_write_ms={:.3} atlas_flush_ms={:.3} atlas_open_ms={:.3} atlas_read_200_ms={:.3} atlas_files={atlas_files} atlas_bytes={atlas_bytes} files_write_ms={:.3} files_open_ms={:.3} files_read_200_ms={:.3} files_count={file_count} files_bytes={file_bytes}",
        millis(atlas_write),
        millis(atlas_flush),
        millis(atlas_open),
        millis(atlas_read),
        millis(files_write),
        millis(files_open),
        millis(files_read)
    );
    Ok(())
}

fn atlas(path: &str, count: usize) -> anyhow::Result<()> {
    let dir = Path::new(path).join("atlas");
    std::fs::create_dir_all(&dir)?;
    let modified = SystemTime::now();
    let names: Vec<String> = (0..count)
        .map(|index| format!("thumb-{index:06}"))
        .collect();
    let frame = test_frame();
    let mut cache = ThumbCache::open(&dir);
    let mut write = Duration::ZERO;
    for (index, name) in names.iter().enumerate() {
        let started = Instant::now();
        anyhow::ensure!(
            cache.put(name, index as u64, modified, &frame),
            "atlas rejected frame"
        );
        write += started.elapsed();
    }
    let flush_started = Instant::now();
    cache.flush()?;
    let flush = flush_started.elapsed();
    drop(cache);
    let open_started = Instant::now();
    let mut cache = ThumbCache::open(&dir);
    let open = open_started.elapsed();
    let read_started = Instant::now();
    for index in read_indices(count) {
        anyhow::ensure!(
            cache.get(&names[index], index as u64, modified).is_some(),
            "atlas miss"
        );
    }
    let read = read_started.elapsed();
    let (files, bytes) = disk_stats(&dir)?;
    println!(
        "count={count} atlas_write_ms={:.3} atlas_flush_ms={:.3} atlas_open_ms={:.3} atlas_read_200_ms={:.3} atlas_files={files} atlas_bytes={bytes}",
        millis(write),
        millis(flush),
        millis(open),
        millis(read)
    );
    Ok(())
}

fn test_frame() -> Frame {
    let width: u32 = 512;
    let height: u32 = 288;
    let mut data = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            data.extend_from_slice(&[x as u8, y as u8, x.wrapping_add(y) as u8, 255]);
        }
    }
    Frame::new(width, height, data)
}

fn read_indices(count: usize) -> Vec<usize> {
    (0..200)
        .map(|index| (index as u64 * 7919 + 104_729) as usize % count.max(1))
        .collect()
}

fn disk_stats(path: &Path) -> anyhow::Result<(usize, u64)> {
    let mut files = 0;
    let mut bytes = 0;
    for entry in std::fs::read_dir(path)?.flatten() {
        if entry.file_type()?.is_file() {
            files += 1;
            bytes += entry.metadata()?.len();
        }
    }
    Ok((files, bytes))
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
