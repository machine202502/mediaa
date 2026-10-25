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

pub fn rgba(size: u32) -> Vec<u8> {
    let n = size as f32;
    let aa = 0.8;
    let mut out = vec![0u8; (size * size * 4) as usize];
    let c = n / 2.0;
    let deep = [22.0, 44.0, 120.0];
    let blue = [37.0, 99.0, 235.0];
    let cyan = [34.0, 211.0, 238.0];
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
pub enum Badge {
    Area,
    Record,
    Stop,
}

pub fn badge(kind: Badge, size: u32) -> Vec<u8> {
    let n = size as f32;
    let c = n / 2.0;
    let aa = 0.7;
    let red = [229.0, 57.0, 53.0];
    let blue = [37.0, 99.0, 235.0];
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (cov, rgb) = match kind {
                Badge::Record => (
                    1.0 - smoothstep(-aa, aa, (px - c).hypot(py - c) - n * 0.34),
                    red,
                ),
                Badge::Stop => (
                    1.0 - smoothstep(-aa, aa, rounded_box_sdf(px, py, c, n * 0.3, n * 0.06)),
                    red,
                ),
                Badge::Area => {
                    let d = rounded_box_sdf(px, py, c, n * 0.36, n * 0.04).abs() - n * 0.05;
                    let near_corner = (px - c).abs() > n * 0.14 && (py - c).abs() > n * 0.14;
                    (
                        if near_corner {
                            1.0 - smoothstep(-aa, aa, d)
                        } else {
                            0.0
                        },
                        blue,
                    )
                }
            };
            let i = ((y * size + x) * 4) as usize;
            out[i..i + 3].copy_from_slice(&[rgb[0] as u8, rgb[1] as u8, rgb[2] as u8]);
            out[i + 3] = (cov.clamp(0.0, 1.0) * 255.0) as u8;
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

pub fn rgba_status(size: u32, status: StatusDot) -> Vec<u8> {
    let mut out = rgba(size);
    let n = size as f32;
    let (cx, cy, r, rim) = (n * 0.74, n * 0.74, n * 0.22, n * 0.06);
    let color = match status {
        StatusDot::Recording => [239.0, 35.0, 35.0],
        StatusDot::Paused => [37.0, 99.0, 235.0],
        StatusDot::Saving => [246.0, 145.0, 28.0],
    };
    for y in 0..size {
        for x in 0..size {
            let d = (x as f32 + 0.5 - cx).hypot(y as f32 + 0.5 - cy);
            let outer = 1.0 - smoothstep(r + rim - 0.8, r + rim + 0.8, d);
            if outer <= 0.0 {
                continue;
            }
            let dot = 1.0 - smoothstep(r - 0.8, r + 0.8, d);
            let rgb = mix([255.0, 255.0, 255.0], color, dot);
            let i = ((y * size + x) * 4) as usize;
            for c in 0..3 {
                out[i + c] = (f32::from(out[i + c]) * (1.0 - outer) + rgb[c] * outer) as u8;
            }
            out[i + 3] = out[i + 3].max((outer * 255.0) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn icon_has_transparent_corners_and_white_marks() {
        let n = 64;
        let px = super::rgba(n);
        assert_eq!(px.len(), (n * n * 4) as usize);
        assert_eq!(px[3], 0, "corner is transparent");
        let (x, y) = ((0.44 * n as f32) as u32, (0.56 * n as f32) as u32);
        let i = ((y * n + x) * 4) as usize;
        assert!(px[i] > 240 && px[i + 1] > 240 && px[i + 2] > 240 && px[i + 3] == 255);
    }

    #[test]
    fn badges_have_the_right_colour_in_the_middle_or_corner() {
        use super::{badge, Badge};
        let px = |v: &[u8], x: u32, y: u32| {
            let i = ((y * 32 + x) * 4) as usize;
            [v[i], v[i + 1], v[i + 2], v[i + 3]]
        };
        assert_eq!(px(&badge(Badge::Record, 32), 16, 16), [229, 57, 53, 255]);
        assert_eq!(px(&badge(Badge::Stop, 32), 16, 16), [229, 57, 53, 255]);
        let area = badge(Badge::Area, 32);
        assert_eq!(px(&area, 16, 16)[3], 0, "area badge is hollow");
        assert!(
            px(&area, 5, 5)[3] > 200,
            "corner mark: {:?}",
            px(&area, 5, 5)
        );
    }

    #[test]
    fn tray_status_dots_have_distinct_colors() {
        let n = 32;
        let sample = |status| {
            let pixels = super::rgba_status(n, status);
            let i = ((24 * n + 24) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        let recording = sample(super::StatusDot::Recording);
        let paused = sample(super::StatusDot::Paused);
        let saving = sample(super::StatusDot::Saving);
        assert!(recording[0] > 200 && recording[1] < 80, "{recording:?}");
        assert!(paused[2] > 200 && paused[0] < 80, "{paused:?}");
        assert!(
            saving[0] > 200 && saving[1] > 100 && saving[2] < 80,
            "{saving:?}"
        );
        assert_eq!(super::rgba_status(n, super::StatusDot::Saving)[3], 0);
    }
}

#[cfg(test)]
#[test]
fn export_icon_png() {
    if let Ok(path) = std::env::var("MEDIAA_ICON_PNG") {
        let img = image::RgbaImage::from_raw(256, 256, rgba(256)).unwrap();
        img.save(path).unwrap();
    }
}
