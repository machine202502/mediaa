use mediaa_core::settings::{AnimSettings, GifDither};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Effort {
    Fast,
    #[default]
    Balanced,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 3] = [Effort::Fast, Effort::Balanced, Effort::Max];

    pub(crate) fn x264_preset(self) -> &'static str {
        match self {
            Effort::Fast => "veryfast",
            Effort::Balanced => "medium",
            Effort::Max => "veryslow",
        }
    }

    pub(crate) fn vp9_cpu_used(self) -> &'static str {
        match self {
            Effort::Fast => "6",
            Effort::Balanced => "4",
            Effort::Max => "1",
        }
    }

    pub(crate) fn webp_level(self) -> &'static str {
        match self {
            Effort::Fast => "2",
            Effort::Balanced => "4",
            Effort::Max => "6",
        }
    }

    pub(crate) fn flac_level(self) -> &'static str {
        match self {
            Effort::Fast => "2",
            Effort::Balanced => "5",
            Effort::Max => "8",
        }
    }

    pub(crate) fn png_preset(self) -> u8 {
        match self {
            Effort::Fast => 1,
            Effort::Balanced => 3,
            Effort::Max => 6,
        }
    }
}

pub fn crf_for_quality(quality: u8) -> u32 {
    let q = u32::from(quality.clamp(1, 100));
    40 - (q - 1) * 24 / 99
}

fn scale_step(max_height: Option<u32>) -> String {
    match max_height {
        Some(h) => format!(",scale=w=-1:h='min({h},ih)':flags=lanczos"),
        None => String::new(),
    }
}

fn dither_option(d: GifDither) -> &'static str {
    match d {
        GifDither::None => "none",
        GifDither::Bayer => "bayer:bayer_scale=3",
        GifDither::Sierra2 => "sierra2",
        GifDither::FloydSteinberg => "floyd_steinberg",
    }
}

pub fn gif_graph(a: &AnimSettings) -> String {
    format!(
        "[0:v]fps={fps}{scale},split[a][b];[a]palettegen=max_colors={colors}:stats_mode=diff[p];\
         [b][p]paletteuse=dither={dither}:diff_mode=rectangle[v]",
        fps = a.fps.clamp(1, 60),
        scale = scale_step(a.height_limit()),
        colors = a.gif_colors.clamp(4, 256),
        dither = dither_option(a.gif_dither),
    )
}

pub fn webp_graph(a: &AnimSettings) -> String {
    format!(
        "[0:v]fps={}{}[v]",
        a.fps.clamp(1, 60),
        scale_step(a.height_limit())
    )
}

pub fn webp_segment_graph(a: &AnimSettings) -> String {
    format!(
        "[0:v]fps={}{},setpts=PTS-STARTPTS[v]",
        a.fps.clamp(1, 60),
        scale_step(a.height_limit())
    )
}

pub fn convert_video_graph(max_height: Option<u32>) -> String {
    let scale = max_height.map_or(String::new(), |h| {
        format!("scale=w=-2:h='min({h},ih)':flags=lanczos,")
    });
    format!("[0:v]{scale}pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p[v]")
}

pub fn still_graph(max_height: Option<u32>, pix_fmt: &str) -> String {
    let scale = max_height.map_or(String::new(), |h| {
        format!("scale=w=-1:h='min({h},ih)':flags=lanczos,")
    });
    format!("[0:v]{scale}format={pix_fmt}[v]")
}

pub fn palette_graph(max_height: Option<u32>, colors: u16) -> String {
    let scale = max_height.map_or(String::new(), |h| {
        format!("scale=w=-1:h='min({h},ih)':flags=lanczos,")
    });
    format!(
        "[0:v]{scale}split[a][b];[a]palettegen=max_colors={c}:reserve_transparent=1:stats_mode=single[p];\
         [b][p]paletteuse=dither=sierra2_4a:alpha_threshold=128[v]",
        c = colors.clamp(2, 256)
    )
}

pub fn vp9_crf(quality: u8) -> u32 {
    let q = u32::from(quality.clamp(1, 100));
    55 - (q - 1) * 40 / 99
}

pub fn qscale(quality: u8) -> i32 {
    let q = i32::from(quality.clamp(1, 100));
    31 - (q - 1) * 29 / 99
}

pub fn live_preset(width: u32, height: u32, fps: u32) -> &'static str {
    let rate = u64::from(width) * u64::from(height) * u64::from(fps);
    if rate > 2560 * 1440 * 60 {
        "superfast"
    } else {
        "veryfast"
    }
}

pub fn scaled_size(width: u32, height: u32, max_height: Option<u32>) -> (u32, u32) {
    match max_height {
        Some(h) if h < height => (
            ((u64::from(width) * u64::from(h) / u64::from(height)) as u32) & !1,
            h,
        ),
        _ => (width, height),
    }
}

pub fn scratch_graph(pix_fmt: &str, max_height: Option<u32>) -> String {
    match max_height {
        Some(h) => format!("[0:v]scale=w=-2:h='min({h},ih)':flags=lanczos,format={pix_fmt}[v]"),
        None => format!("[0:v]format={pix_fmt}[v]"),
    }
}

pub fn audio_only_mix_graph(n: usize) -> String {
    audio_mix_graph_range(n, 0)
}

pub fn audio_mix_graph(n: usize) -> Option<String> {
    (n > 0).then(|| audio_mix_graph_range(n, 1))
}

fn audio_mix_graph_range(n: usize, first: usize) -> String {
    match n {
        0 | 1 => format!("[{first}:a]aresample=async=1[a]"),
        _ => {
            let inputs: String = (first..first + n)
                .map(|index| format!("[{index}:a]"))
                .collect();
            format!("{inputs}amix=inputs={n}:duration=longest:normalize=0[a]")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_graphs_and_scales() {
        assert_eq!(
            convert_video_graph(None),
            "[0:v]pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p[v]"
        );
        assert_eq!(
            convert_video_graph(Some(720)),
            "[0:v]scale=w=-2:h='min(720,ih)':flags=lanczos,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p[v]"
        );
        assert_eq!(still_graph(None, "rgba"), "[0:v]format=rgba[v]");
        assert_eq!(
            still_graph(Some(1080), "yuvj420p"),
            "[0:v]scale=w=-1:h='min(1080,ih)':flags=lanczos,format=yuvj420p[v]"
        );
        assert_eq!(
            palette_graph(Some(720), 64),
            "[0:v]scale=w=-1:h='min(720,ih)':flags=lanczos,split[a][b];[a]palettegen=max_colors=64:reserve_transparent=1:stats_mode=single[p];\
             [b][p]paletteuse=dither=sierra2_4a:alpha_threshold=128[v]"
        );
        assert!(
            palette_graph(None, 999).starts_with("[0:v]split[a][b];[a]palettegen=max_colors=256:")
        );
        assert_eq!((vp9_crf(100), vp9_crf(1)), (15, 55));
        assert_eq!((qscale(100), qscale(1)), (2, 31));
        assert!(
            (1..100).all(|q| vp9_crf(q) >= vp9_crf(q + 1) && qscale(q) >= qscale(q + 1)),
            "better quality never costs less"
        );
    }

    #[test]
    fn crf_mapping_is_monotonic_and_bounded() {
        assert_eq!(crf_for_quality(100), 16);
        assert_eq!(crf_for_quality(1), 40);
        assert_eq!(crf_for_quality(0), 40);
        let mut prev = u32::MAX;
        for q in 1..=100 {
            let c = crf_for_quality(q);
            assert!(c <= prev);
            prev = c;
        }
    }

    #[test]
    fn gif_graph_default() {
        let g = gif_graph(&AnimSettings::default());
        assert_eq!(
            g,
            "[0:v]fps=15,scale=w=-1:h='min(480,ih)':flags=lanczos,split[a][b];[a]palettegen=max_colors=256:stats_mode=diff[p];\
             [b][p]paletteuse=dither=sierra2:diff_mode=rectangle[v]",
            "480p by default"
        );
    }

    #[test]
    fn gif_graph_with_scaling_and_bayer() {
        let a = AnimSettings {
            fps: 10,
            max_height: 360,
            gif_colors: 64,
            gif_dither: GifDither::Bayer,
            ..AnimSettings::default()
        };
        let g = gif_graph(&a);
        assert!(
            g.starts_with("[0:v]fps=10,scale=w=-1:h='min(360,ih)':flags=lanczos,split"),
            "{g}"
        );
        assert!(g.contains("max_colors=64"));
        assert!(g.contains("dither=bayer:bayer_scale=3"));
        assert!(g.ends_with("[v]"));
    }

    #[test]
    fn webp_graph_variants() {
        let original = AnimSettings {
            max_height: 0,
            ..AnimSettings::default()
        };
        assert_eq!(webp_graph(&original), "[0:v]fps=15[v]");
        assert_eq!(
            webp_graph(&AnimSettings::default()),
            "[0:v]fps=15,scale=w=-1:h='min(480,ih)':flags=lanczos[v]",
            "480p by default"
        );
        assert_eq!(
            webp_segment_graph(&AnimSettings::default()),
            "[0:v]fps=15,scale=w=-1:h='min(480,ih)':flags=lanczos,setpts=PTS-STARTPTS[v]"
        );
    }

    #[test]
    fn scratch_graphs() {
        assert_eq!(scratch_graph("yuv420p", None), "[0:v]format=yuv420p[v]");
        assert_eq!(
            scratch_graph("bgr0", Some(1080)),
            "[0:v]scale=w=-2:h='min(1080,ih)':flags=lanczos,format=bgr0[v]"
        );
    }

    #[test]
    fn live_preset_and_scaled_size() {
        assert_eq!(scaled_size(3200, 2000, Some(1080)), (1728, 1080));
        assert_eq!(
            scaled_size(1280, 720, Some(1080)),
            (1280, 720),
            "never upscales"
        );
        assert_eq!(live_preset(1920, 1080, 60), "veryfast");
        assert_eq!(live_preset(2560, 1440, 60), "veryfast");
        assert_eq!(live_preset(3200, 2000, 60), "superfast");
        assert_eq!(live_preset(3200, 2000, 30), "veryfast");
    }

    #[test]
    fn audio_graphs() {
        assert_eq!(audio_only_mix_graph(1), "[0:a]aresample=async=1[a]");
        assert_eq!(
            audio_only_mix_graph(2),
            "[0:a][1:a]amix=inputs=2:duration=longest:normalize=0[a]"
        );
        assert_eq!(audio_mix_graph(0), None);
        assert_eq!(audio_mix_graph(1).unwrap(), "[1:a]aresample=async=1[a]");
        assert_eq!(
            audio_mix_graph(2).unwrap(),
            "[1:a][2:a]amix=inputs=2:duration=longest:normalize=0[a]"
        );
    }
}
