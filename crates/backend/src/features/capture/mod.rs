use mediaa_core::ImageFormat;
use mediaa_platform::screen::{self, CaptureWindowId};
use mediaa_protocol::backend::{
    CaptureSourceWire, CaptureTargetWire, ScreenshotCommitWire, ShotResultWire,
};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 4] = b"MCA1";
const CAPTURE_DIR: &str = "capture";

fn capture_dir(media_dir: &Path) -> PathBuf {
    media_dir.join(".cache").join(CAPTURE_DIR)
}

fn write_rgba(path: &Path, frame: &mediaa_core::Frame) -> anyhow::Result<()> {
    let mut out = Vec::with_capacity(16 + frame.data.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&(frame.width * 4).to_le_bytes());
    out.extend_from_slice(&frame.data);
    std::fs::write(path, out)?;
    Ok(())
}

fn read_rgba(path: &Path) -> anyhow::Result<mediaa_core::Frame> {
    let data = std::fs::read(path)?;
    anyhow::ensure!(data.len() >= 16 && &data[..4] == MAGIC, "bad capture file");
    let width = u32::from_le_bytes(data[4..8].try_into().expect("width"));
    let height = u32::from_le_bytes(data[8..12].try_into().expect("height"));
    let stride = u32::from_le_bytes(data[12..16].try_into().expect("stride"));
    anyhow::ensure!(stride == width * 4, "bad capture stride");
    let pixels = data[16..].to_vec();
    anyhow::ensure!(
        pixels.len() == (width as usize) * (height as usize) * 4,
        "bad capture size"
    );
    Ok(mediaa_core::Frame {
        width,
        height,
        data: pixels,
    })
}

pub fn capture_target_file(
    media_dir: &Path,
    target: CaptureTargetWire,
) -> Result<CaptureSourceWire, String> {
    let (frame, scale) = match target {
        CaptureTargetWire::Monitor { index } => {
            let monitors = screen::monitors().map_err(|error| format!("{error:#}"))?;
            let monitor = monitors
                .get(index)
                .ok_or_else(|| "monitor not found".to_string())?;
            let frame =
                screen::capture_monitor(monitor.id).map_err(|error| format!("{error:#}"))?;
            (frame, monitor.scale)
        }
        CaptureTargetWire::Window { id } => {
            let frame = screen::capture_window(CaptureWindowId::new(id))
                .map_err(|error| format!("{error:#}"))?;
            (frame, 1.0)
        }
    };
    let dir = capture_dir(media_dir);
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let path = dir.join(format!("cap-{stamp}.rgba"));
    write_rgba(&path, &frame).map_err(|error| format!("{error:#}"))?;
    let path = path.to_string_lossy().into_owned();
    Ok(CaptureSourceWire {
        work: path.clone(),
        path,
        width: frame.width,
        height: frame.height,
        scale,
    })
}

pub fn commit_screenshot(
    commit: &ScreenshotCommitWire,
    clipboard: &crate::features::clipboard::ClipboardHandle,
) -> Result<ShotResultWire, String> {
    let source = PathBuf::from(&commit.source);
    let frame = read_rgba(&source).map_err(|error| format!("{error:#}"))?;
    let size = format!("{}×{}", frame.width, frame.height);
    let format = match commit.format.as_str() {
        "png" => ImageFormat::Png,
        "webp" => ImageFormat::Webp,
        other => return Err(format!("unsupported format: {other}")),
    };
    let output = commit.output.as_ref().map(PathBuf::from);
    if let Some(output) = output.as_ref() {
        mediaa_storage::screenshot::save(&frame, format, output)
            .map_err(|error| format!("{error:#}"))?;
    }
    if commit.copy_image {
        clipboard.copy_image_wait(frame)?;
    }
    if commit.copy_file {
        let output = output
            .clone()
            .ok_or_else(|| "no output path for a file copy".to_string())?;
        clipboard
            .copy_files_wait(vec![output])
            .map_err(|error| error.to_string())?;
    }
    let _ = std::fs::remove_file(&source);
    Ok(ShotResultWire {
        path: output.map(|path| path.to_string_lossy().into_owned()),
        size,
        copied: commit.copy_image || commit.copy_file,
    })
}
