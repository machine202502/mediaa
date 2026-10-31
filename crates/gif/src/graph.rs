use mediaa_core::limits::{FPS, GIF_COLORS};
use mediaa_core::settings::{AnimSettings, GifDither};

fn scale_step(max_height: Option<u32>) -> String {
    match max_height {
        Some(h) => format!(",scale=w=-1:h='min({h},ih)':flags=lanczos"),
        None => String::new(),
    }
}

fn dither_option(dither: GifDither) -> &'static str {
    match dither {
        GifDither::None => "none",
        GifDither::Bayer => "bayer:bayer_scale=3",
        GifDither::Sierra2 => "sierra2",
        GifDither::FloydSteinberg => "floyd_steinberg",
    }
}

pub fn graph(settings: &AnimSettings) -> String {
    format!(
        "[0:v]fps={fps}{scale},split[a][b];[a]palettegen=max_colors={colors}:stats_mode=diff[p];\
         [b][p]paletteuse=dither={dither}:diff_mode=rectangle[v]",
        fps = FPS.clamp(settings.fps),
        scale = scale_step(settings.height_limit()),
        colors = GIF_COLORS.clamp(settings.gif_colors),
        dither = dither_option(settings.gif_dither),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_graph_uses_sierra2_and_480p() {
        let graph = graph(&AnimSettings::default());
        assert!(graph.starts_with("[0:v]fps=15,scale=w=-1:h='min(480,ih)':flags=lanczos,"));
        assert!(graph.contains("palettegen=max_colors=256:stats_mode=diff"));
        assert!(graph.contains("paletteuse=dither=sierra2:diff_mode=rectangle"));
    }

    #[test]
    fn bayer_and_native_height_are_respected() {
        let settings = AnimSettings {
            gif_dither: GifDither::Bayer,
            max_height: 0,
            ..AnimSettings::default()
        };
        let graph = graph(&settings);
        assert!(graph.starts_with("[0:v]fps=15,split"));
        assert!(graph.contains("dither=bayer:bayer_scale=3"));
    }
}
