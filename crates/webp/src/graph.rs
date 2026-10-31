use mediaa_core::limits::FPS;
use mediaa_core::settings::AnimSettings;

fn scale_step(max_height: Option<u32>) -> String {
    match max_height {
        Some(h) => format!(",scale=w=-1:h='min({h},ih)':flags=lanczos"),
        None => String::new(),
    }
}

pub fn graph(settings: &AnimSettings) -> String {
    format!(
        "[0:v]fps={}{}[v]",
        FPS.clamp(settings.fps),
        scale_step(settings.height_limit())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_scaled_graphs() {
        assert_eq!(
            graph(&AnimSettings::default()),
            "[0:v]fps=15,scale=w=-1:h='min(480,ih)':flags=lanczos[v]"
        );
        let native = AnimSettings {
            max_height: 0,
            ..AnimSettings::default()
        };
        assert_eq!(graph(&native), "[0:v]fps=15[v]");
    }
}
