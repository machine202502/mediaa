use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect as ERect, Stroke, StrokeKind, ViewportId};
use mediaa_core::geometry::Rect;
use mediaa_platform::screen::MonitorInfo;

const TITLE: &str = "MediaA recording area";
const GAP: f32 = 4.0;
const RECORDING: Color32 = Color32::from_rgb(255, 70, 82);
const RECORDING_DIM: Color32 = Color32::from_rgb(154, 35, 46);
const PAUSED: Color32 = Color32::from_rgb(70, 148, 255);

#[derive(Debug, Clone, Copy, PartialEq)]
struct Geometry {
    position: [f32; 2],
    size: [f32; 2],
}

fn geometry(monitor: &MonitorInfo, rect: Rect, width: u32) -> Geometry {
    let scale = monitor.scale.max(0.1);
    let margin = width.clamp(2, 32) as f32 + GAP;
    Geometry {
        position: [
            (monitor.x + rect.x) as f32 / scale - margin,
            (monitor.y + rect.y) as f32 / scale - margin,
        ],
        size: [rect.w.max(1) as f32 / scale + margin * 2.0, rect.h.max(1) as f32 / scale + margin * 2.0],
    }
}

pub fn show(
    ctx: &egui::Context,
    monitor: &MonitorInfo,
    rect: Rect,
    paused: bool,
    blink_on: bool,
    width: u32,
    native: &mut mediaa_platform::outline::RegionOutline,
) {
    let scale = monitor.scale.max(0.1);
    let width = width.clamp(2, 32);
    let margin = ((width as f32 + GAP) * scale).round() as i32;
    let stroke = (width as f32 * scale).round() as i32;
    if native.show(
        mediaa_platform::outline::OutlineGeometry {
            x: monitor.x + rect.x - margin,
            y: monitor.y + rect.y - margin,
            width: rect.w.max(1) + margin * 2,
            height: rect.h.max(1) + margin * 2,
            stroke_width: stroke,
        },
        if paused {
            mediaa_platform::outline::OutlineAppearance::Paused
        } else if blink_on {
            mediaa_platform::outline::OutlineAppearance::Active
        } else {
            mediaa_platform::outline::OutlineAppearance::ActiveDim
        },
    ) {
        return;
    }
    let geometry = geometry(monitor, rect, width);
    let id = ViewportId::from_hash_of("mediaa-recording-region");
    let builder = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_position(geometry.position)
        .with_inner_size(geometry.size)
        .with_decorations(false)
        .with_resizable(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_mouse_passthrough(true)
        .with_taskbar(false)
        .with_active(false);
    ctx.show_viewport_deferred(id, builder, move |ui, _| {
        let bounds = ERect::from_min_size(Pos2::ZERO, ui.available_size());
        let color = if paused {
            PAUSED
        } else if blink_on {
            RECORDING
        } else {
            RECORDING_DIM
        };
        ui.painter().rect_stroke(bounds, CornerRadius::ZERO, Stroke::new(width as f32, color), StrokeKind::Inside);
    });
    let _ = mediaa_platform::window::exclude_from_capture(TITLE);
    let _ = mediaa_platform::window::make_black_transparent(TITLE);
}

#[cfg(test)]
mod tests {
    use super::{geometry, Geometry, GAP};
    use mediaa_core::geometry::Rect;
    use mediaa_platform::screen::MonitorInfo;

    #[test]
    fn marker_uses_monitor_scale_and_stays_outside_the_recorded_edge() {
        let monitor = MonitorInfo {
            id: 1,
            device: "display".into(),
            index: 0,
            name: "Display".into(),
            x: 100,
            y: 50,
            width: 1600,
            height: 900,
            scale: 2.0,
            primary: true,
        };
        assert_eq!(
            geometry(&monitor, Rect::new(10, 20, 300, 200), 4),
            Geometry {
                position: [47.0, 27.0],
                size: [166.0, 116.0],
            }
        );
        assert!(geometry(&monitor, Rect::new(0, 0, 1, 1), 32).size[0] > (32.0 + GAP) * 2.0);
    }
}
