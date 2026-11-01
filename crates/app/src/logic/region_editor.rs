use mediaa_core::geometry::Rect;
use mediaa_core::settings::AfterCapture;
use mediaa_core::{Frame, ImageFormat};
use mediaa_platform::screen::MonitorInfo;
use mediaa_render::annotate::Shape;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Purpose {
    Screenshot,
    Area,
    RecordRegion,
}

#[derive(Debug)]
pub(crate) enum Outcome {
    Cancel,
    Image {
        frame: Frame,
        selection: Rect,
        shapes: Vec<Shape>,
        action: AfterCapture,
        format: Option<ImageFormat>,
        after_completion: bool,
    },
    Saved {
        path: PathBuf,
        size: String,
    },
    SelectedRegion {
        monitor: MonitorInfo,
        rect: Rect,
    },
    RecordArea {
        monitor: MonitorInfo,
        rect: Rect,
    },
    CaptureMonitor {
        index: usize,
        monitor: MonitorInfo,
    },
}

#[derive(Clone)]
pub(crate) struct SaveDefaults {
    pub dir: PathBuf,
    pub format: ImageFormat,
}

pub(crate) fn resolve_save_path(
    path: PathBuf,
    fallback: ImageFormat,
) -> Option<(PathBuf, ImageFormat)> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension {
        None => Some((path.with_extension(fallback.extension()), fallback)),
        Some(extension) => ImageFormat::ALL
            .iter()
            .find(|format| format.extension() == extension)
            .map(|format| (path, *format)),
    }
}

pub(crate) struct SavedShot {
    pub path: PathBuf,
    pub size: String,
}

pub(crate) fn save_crop(
    frame: &Frame,
    selection: Rect,
    shapes: &[Shape],
    format: ImageFormat,
    path: PathBuf,
) -> Result<SavedShot, String> {
    let cropped = mediaa_render::annotate::render_crop(frame, selection, shapes);
    let size = format!("{}×{}", cropped.width, cropped.height);
    mediaa_storage::screenshot::save(&cropped, format, &path)
        .map_err(|error| format!("{error:#}"))?;
    Ok(SavedShot { path, size })
}

pub(crate) fn save_formats(preferred: ImageFormat) -> [ImageFormat; 2] {
    if preferred == ImageFormat::Png {
        [ImageFormat::Png, ImageFormat::Webp]
    } else {
        [ImageFormat::Webp, ImageFormat::Png]
    }
}

pub(crate) fn save_stem(
    dir: &Path,
    format: ImageFormat,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    mediaa_core::naming::media_path(dir, "Screenshot", now, format.extension())
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Screenshot")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn the_extension_of_the_save_path_decides_the_format() {
        assert_eq!(
            resolve_save_path(PathBuf::from("/a/shot.png"), ImageFormat::Webp),
            Some((PathBuf::from("/a/shot.png"), ImageFormat::Png))
        );
        assert_eq!(
            resolve_save_path(PathBuf::from("/a/shot.WEBP"), ImageFormat::Png),
            Some((PathBuf::from("/a/shot.WEBP"), ImageFormat::Webp))
        );
        let (path, format) = resolve_save_path(PathBuf::from("/a/shot"), ImageFormat::Webp)
            .expect("the default format is added");
        assert_eq!(format, ImageFormat::Webp);
        assert_eq!(path, PathBuf::from("/a/shot.webp"));
        assert_eq!(
            resolve_save_path(PathBuf::from("/a/shot.jpg"), ImageFormat::Png),
            None,
            "only PNG and WebP are accepted"
        );
    }

    #[test]
    fn saving_a_crop_writes_the_selected_area() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        let frame = Frame::filled(8, 6, [10, 20, 30, 255]);
        let saved = save_crop(
            &frame,
            Rect::new(2, 1, 4, 3),
            &[],
            ImageFormat::Png,
            path.clone(),
        )
        .expect("saved");
        assert_eq!(saved.size, "4×3");
        assert_eq!(saved.path, path);
        assert_eq!(image::image_dimensions(&saved.path).unwrap(), (4, 3));
    }

    #[test]
    fn the_preferred_format_goes_first() {
        assert_eq!(
            save_formats(ImageFormat::Png),
            [ImageFormat::Png, ImageFormat::Webp]
        );
        assert_eq!(
            save_formats(ImageFormat::Webp),
            [ImageFormat::Webp, ImageFormat::Png]
        );
    }
}
