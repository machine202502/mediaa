use mediaa_core::formats::{AnimFormat, AudioFormat};
use mediaa_media::convert::{self, ConvertSettings, Effort, StillFormat, Target, VideoFormat};
use std::sync::atomic::AtomicBool;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (input, output) = (args.next().expect("input"), args.next().expect("output"));
    let ext = std::path::Path::new(&output).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let target = VideoFormat::ALL
        .map(Target::Video)
        .into_iter()
        .chain(AnimFormat::ALL.map(Target::Anim))
        .chain(StillFormat::ALL.map(Target::Still))
        .chain(AudioFormat::ALL.map(Target::Audio))
        .find(|t| t.extension() == ext)
        .ok_or_else(|| anyhow::anyhow!("unknown output format .{ext}"))?;
    let mut settings = ConvertSettings::default();
    for a in args {
        if a == "--max" {
            settings.effort = Effort::Max;
        } else if let Some(n) = a.strip_prefix("--colors=") {
            settings.png_colors = Some(n.parse()?);
        }
    }
    convert::convert(input.as_ref(), output.as_ref(), target, &settings, &mut |_| {}, &AtomicBool::new(false))?;
    println!("{output}");
    Ok(())
}
