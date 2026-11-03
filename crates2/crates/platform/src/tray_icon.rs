use crate::tray::TrayIcon;
use std::f32::consts::TAU;

const DEFAULT_SIZE: u32 = 32;
const MAX_SCALE: u32 = 8;

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn rounded_box_sdf(px: f32, py: f32, c: f32, half: f32, r: f32) -> f32 {
    let qx = (px - c).abs() - half + r;
    let qy = (py - c).abs() - half + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn triangle_sdf(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    let edge = |p0: (f32, f32), p1: (f32, f32)| {
        let (ex, ey) = (p1.0 - p0.0, p1.1 - p0.1);
        let (wx, wy) = (p.0 - p0.0, p.1 - p0.1);
        let t = ((wx * ex + wy * ey) / (ex * ex + ey * ey)).clamp(0.0, 1.0);
        let d = (wx - ex * t).hypot(wy - ey * t);
        let cross = ex * wy - ey * wx;
        (d, cross)
    };
    let (d0, c0) = edge(a, b);
    let (d1, c1) = edge(b, c);
    let (d2, c2) = edge(c, a);
    let inside = (c0 >= 0.0 && c1 >= 0.0 && c2 >= 0.0) || (c0 <= 0.0 && c1 <= 0.0 && c2 <= 0.0);
    let d = d0.min(d1).min(d2);
    if inside {
        -d
    } else {
        d
    }
}

fn sparkle(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let (dx, dy) = ((px - cx).abs() / r, (py - cy).abs() / r);
    let v = dx.powf(2.0 / 3.0) + dy.powf(2.0 / 3.0);
    let e = 1.5 / r;
    1.0 - smoothstep(1.0 - e, 1.0 + e, v)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

fn base_rgba(size: u32, deep: [f32; 3], blue: [f32; 3], cyan: [f32; 3]) -> Vec<u8> {
    let n = size as f32;
    let aa = 0.8;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let c = n / 2.0;
    let white = [255.0, 255.0, 255.0];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = ((y * size + x) * 4) as usize;
            let tile = 1.0 - smoothstep(-aa, aa, rounded_box_sdf(px, py, c, n * 0.48, n * 0.24));
            if tile <= 0.0 {
                continue;
            }
            let t = ((px - py) / n * 0.5 + 0.5).clamp(0.0, 1.0);
            let mut rgb = if t < 0.5 {
                mix(deep, blue, t * 2.0)
            } else {
                mix(blue, cyan, (t - 0.5) * 2.0)
            };

            let (lx, ly) = (n * 0.44, n * 0.56);
            let d = (px - lx).hypot(py - ly);
            let ring_r = n * 0.25;
            let ring_w = n * 0.055;
            let ring = 1.0 - smoothstep(ring_w - aa, ring_w + aa, (d - ring_r).abs());
            let s = n * 0.11;
            let tri = triangle_sdf(
                (px, py),
                (lx - s * 0.75, ly - s),
                (lx - s * 0.75, ly + s),
                (lx + s * 1.05, ly),
            );
            let play = 1.0 - smoothstep(-aa, aa, tri);
            let spark = sparkle(px, py, n * 0.74, n * 0.27, n * 0.16).max(sparkle(
                px,
                py,
                n * 0.86,
                n * 0.46,
                n * 0.065,
            ));

            let w = ring.max(play).max(spark);
            rgb = mix(rgb, white, w);
            out[i] = rgb[0] as u8;
            out[i + 1] = rgb[1] as u8;
            out[i + 2] = rgb[2] as u8;
            out[i + 3] = (tile * 255.0) as u8;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusDot {
    Recording,
    Paused,
    Saving,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    pub dot: StatusDot,
    pub fraction: Option<f32>,
}

fn dot_color(dot: StatusDot) -> [f32; 3] {
    match dot {
        StatusDot::Recording => [239.0, 35.0, 35.0],
        StatusDot::Paused => [37.0, 99.0, 235.0],
        StatusDot::Saving => [246.0, 145.0, 28.0],
    }
}

pub fn icon(agent: bool, scale: u32) -> TrayIcon {
    let size = DEFAULT_SIZE * scale.clamp(1, MAX_SCALE);
    let rgba = if agent {
        base_rgba(
            size,
            [96.0, 12.0, 12.0],
            [222.0, 34.0, 34.0],
            [255.0, 170.0, 72.0],
        )
    } else {
        base_rgba(
            size,
            [22.0, 44.0, 120.0],
            [37.0, 99.0, 235.0],
            [34.0, 211.0, 238.0],
        )
    };
    TrayIcon {
        width: size,
        height: size,
        rgba,
    }
}

pub fn with_progress(base: &TrayIcon, progress: Progress) -> TrayIcon {
    let mut out = base.rgba.clone();
    let expected = base.width as usize * base.height as usize * 4;
    if base.width == 0 || base.height == 0 || out.len() != expected {
        return base.clone();
    }
    let n = base.width.min(base.height) as f32;
    let (cx, cy) = (n * 0.66, n * 0.66);
    let color = dot_color(progress.dot);
    let r = n * 0.17;
    let rim = n * 0.045;

    if let Some(fraction) = progress.fraction.filter(|value| value.is_finite()) {
        let fraction = fraction.clamp(0.0, 1.0);
        if fraction > 0.0 {
            let ring_r = n * 0.245;
            let ring_w = n * 0.032;
            for y in 0..base.height {
                for x in 0..base.width {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let (dx, dy) = (px - cx, py - cy);
                    let d = dx.hypot(dy);
                    let band = 1.0 - smoothstep(ring_w - 0.6, ring_w + 0.6, (d - ring_r).abs());
                    if band <= 0.0 {
                        continue;
                    }
                    let angle = dx.atan2(-dy).rem_euclid(TAU);
                    if angle > fraction * TAU {
                        continue;
                    }
                    let i = ((y * base.width + x) * 4) as usize;
                    for channel in 0..3 {
                        out[i + channel] =
                            (f32::from(out[i + channel]) * (1.0 - band) + 255.0 * band) as u8;
                    }
                    out[i + 3] = out[i + 3].max((band * 255.0) as u8);
                }
            }
        }
    }

    for y in 0..base.height {
        for x in 0..base.width {
            let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
            let outer = 1.0 - smoothstep(r + rim - 0.8, r + rim + 0.8, d);
            if outer <= 0.0 {
                continue;
            }
            let dot = 1.0 - smoothstep(r - 0.8, r + 0.8, d);
            let rgb = mix([255.0, 255.0, 255.0], color, dot);
            let i = ((y * base.width + x) * 4) as usize;
            for channel in 0..3 {
                out[i + channel] =
                    (f32::from(out[i + channel]) * (1.0 - outer) + rgb[channel] * outer) as u8;
            }
            out[i + 3] = out[i + 3].max((outer * 255.0) as u8);
        }
    }

    TrayIcon {
        width: base.width,
        height: base.height,
        rgba: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(icon: &TrayIcon, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * icon.width + x) * 4) as usize;
        [
            icon.rgba[i],
            icon.rgba[i + 1],
            icon.rgba[i + 2],
            icon.rgba[i + 3],
        ]
    }

    #[test]
    fn icon_has_transparent_corners_and_white_marks() {
        let icon = icon(false, 1);
        assert_eq!((icon.width, icon.height), (32, 32));
        assert_eq!(icon.rgba.len(), 32 * 32 * 4);
        assert_eq!(pixel(&icon, 0, 0)[3], 0);
        let mark = pixel(&icon, 14, 18);
        assert!(
            mark[0] > 240 && mark[1] > 240 && mark[2] > 240 && mark[3] == 255,
            "{mark:?}"
        );
    }

    #[test]
    fn scale_multiplies_the_edge() {
        let doubled = icon(false, 2);
        assert_eq!((doubled.width, doubled.height), (64, 64));
        assert_eq!(doubled.rgba.len(), 64 * 64 * 4);
        let zero = icon(false, 0);
        assert_eq!((zero.width, zero.height), (32, 32));
    }

    #[test]
    fn agent_icon_is_red_and_differs_from_the_normal_one() {
        let normal = icon(false, 2);
        let agent = icon(true, 2);
        assert_ne!(normal.rgba, agent.rgba);
        let mut red = 0u64;
        let mut green = 0u64;
        let mut blue = 0u64;
        for p in agent.rgba.as_chunks::<4>().0 {
            if p[3] > 0 {
                red += u64::from(p[0]);
                green += u64::from(p[1]);
                blue += u64::from(p[2]);
            }
        }
        assert!(red > green * 2, "red {red}, green {green}");
        assert!(red > blue * 2, "red {red}, blue {blue}");
    }

    #[test]
    fn status_dots_have_distinct_colours() {
        let base = icon(false, 1);
        let recording = with_progress(
            &base,
            Progress {
                dot: StatusDot::Recording,
                fraction: None,
            },
        );
        let paused = with_progress(
            &base,
            Progress {
                dot: StatusDot::Paused,
                fraction: None,
            },
        );
        let saving = with_progress(
            &base,
            Progress {
                dot: StatusDot::Saving,
                fraction: None,
            },
        );
        let r = pixel(&recording, 21, 21);
        let p = pixel(&paused, 21, 21);
        let s = pixel(&saving, 21, 21);
        assert!(r[0] > 200 && r[1] < 80, "{r:?}");
        assert!(p[2] > 200 && p[0] < 80, "{p:?}");
        assert!(s[0] > 200 && s[1] > 100 && s[2] < 80, "{s:?}");
        assert_eq!(r[3], 255);
    }

    #[test]
    fn progress_arc_follows_the_fraction_and_keeps_the_base_without_it() {
        let base = icon(false, 1);
        let plain = with_progress(
            &base,
            Progress {
                dot: StatusDot::Recording,
                fraction: None,
            },
        );
        let quarter = with_progress(
            &base,
            Progress {
                dot: StatusDot::Recording,
                fraction: Some(0.25),
            },
        );
        let full = with_progress(
            &base,
            Progress {
                dot: StatusDot::Recording,
                fraction: Some(1.0),
            },
        );
        let bottom = (21, 29);
        let right = (28, 20);
        let left = (13, 21);
        assert_eq!(
            pixel(&quarter, right.0, right.1),
            pixel(&full, right.0, right.1)
        );
        assert_ne!(
            pixel(&quarter, bottom.0, bottom.1),
            pixel(&full, bottom.0, bottom.1)
        );
        assert_eq!(
            pixel(&quarter, left.0, left.1),
            pixel(&plain, left.0, left.1)
        );
        assert_eq!((plain.width, plain.height), (base.width, base.height));
    }
}
