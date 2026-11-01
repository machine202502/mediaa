use mediaa_core::settings::AfterCapture;
use mediaa_core::{naming, ImageFormat};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ShotPlan {
    pub format: ImageFormat,
    pub path: Option<PathBuf>,
    pub copy_image: bool,
    pub copy_file: bool,
}

pub(crate) fn plan(
    action: AfterCapture,
    format: Option<ImageFormat>,
    after_completion: bool,
    media_dir: &Path,
    now: chrono::DateTime<chrono::Local>,
) -> ShotPlan {
    let format = format.unwrap_or(ImageFormat::Png);
    let copy_file = after_completion;
    let save = action.saves() || after_completion;
    let copy_image = action.copies() && !copy_file;
    let path = save.then(|| naming::media_path(media_dir, "Screenshot", now, format.extension()));
    ShotPlan {
        format,
        path,
        copy_image,
        copy_file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn planned(action: AfterCapture, after_completion: bool) -> ShotPlan {
        plan(
            action,
            None,
            after_completion,
            Path::new("C:/media"),
            chrono::Local::now(),
        )
    }

    #[test]
    fn saving_action_writes_a_png_and_copies_nothing_else() {
        let plan = planned(AfterCapture::Save, false);
        assert!(plan.path.is_some());
        assert_eq!(plan.format, ImageFormat::Png);
        assert!(!plan.copy_image);
        assert!(!plan.copy_file);
    }

    #[test]
    fn copying_action_keeps_the_image_in_the_clipboard() {
        let plan = planned(AfterCapture::Copy, false);
        assert!(plan.path.is_none());
        assert!(plan.copy_image);
        assert!(!plan.copy_file);
    }

    #[test]
    fn copy_and_save_writes_the_file_and_copies_the_image() {
        let plan = planned(AfterCapture::CopyAndSave, false);
        assert!(plan.path.is_some());
        assert!(plan.copy_image);
        assert!(!plan.copy_file);
    }

    #[test]
    fn tray_capture_saves_and_copies_the_file_instead_of_the_image() {
        let plan = planned(AfterCapture::Save, true);
        assert!(plan.path.is_some());
        assert!(!plan.copy_image);
        assert!(plan.copy_file);
    }

    #[test]
    fn the_chosen_format_decides_the_extension() {
        let plan = plan(
            AfterCapture::Save,
            Some(ImageFormat::Webp),
            false,
            Path::new("C:/media"),
            chrono::Local::now(),
        );
        let path = plan.path.expect("saved path");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("webp"));
    }
}
