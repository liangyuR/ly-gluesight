//! 示教 MCP 的叠图：原图裁剪、缩放成 RGB，画原图坐标网格与测量线，编码成 PNG 给 Agent 看。

use crate::frame::FrameImage;

pub type Rgb = [u8; 3];
pub const CYAN: Rgb = [0, 230, 255];
pub const YELLOW: Rgb = [255, 220, 0];
pub const GREEN: Rgb = [40, 255, 90];
pub const MAGENTA: Rgb = [255, 60, 220];
const BLACK: Rgb = [0, 0, 0];
const WHITE: Rgb = [255, 255, 255];
/// 放大看局部时最多放大的倍数
const MAX_ZOOM: f64 = 4.0;

/// 3×5 点阵数字，每行 3 位、自上而下
const DIGITS: [u16; 10] = [
    0b111_101_101_101_111,
    0b010_110_010_010_111,
    0b111_001_111_100_111,
    0b111_001_111_001_111,
    0b101_101_111_001_001,
    0b111_100_111_001_111,
    0b111_100_111_101_111,
    0b111_001_001_001_001,
    0b111_101_111_101_111,
    0b111_101_111_001_111,
];
const GLYPH: usize = 2;

pub struct Canvas {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    /// 画布左上角对应的原图坐标
    origin: [f64; 2],
    /// 画布像素 / 原图像素
    scale: f64,
    source: [u32; 2],
}

impl Canvas {
    /// `crop` 为原图像素 [x, y, w, h]（裁到图内），输出长边不超过 `max_side`；小区域最多放大 4 倍。
    pub fn new(image: &FrameImage, crop: Option<[u32; 4]>, max_side: u32) -> Result<Self, String> {
        let [x0, y0, w, h] = clip(crop, image.width, image.height)?;
        let scale = (f64::from(max_side.max(64)) / f64::from(w.max(h))).min(MAX_ZOOM);
        let width = ((f64::from(w) * scale).round() as u32).max(1);
        let height = ((f64::from(h) * scale).round() as u32).max(1);
        let channels = usize::from(image.channels);
        let at = |x: u32, y: u32| (y as usize * image.width as usize + x as usize) * channels;
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 3);
        for oy in 0..height {
            let (sy0, sy1) = footprint(oy, scale, y0, y0 + h);
            for ox in 0..width {
                let (sx0, sx1) = footprint(ox, scale, x0, x0 + w);
                let mut sum = [0u32; 3];
                for sy in sy0..sy1 {
                    for sx in sx0..sx1 {
                        let i = at(sx, sy);
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += u32::from(image.pixels[i + if channels == 3 { c } else { 0 }]);
                        }
                    }
                }
                let n = (sy1 - sy0) * (sx1 - sx0);
                pixels.extend(sum.map(|s| (s / n) as u8));
            }
        }
        Ok(Self {
            width,
            height,
            pixels,
            origin: [f64::from(x0), f64::from(y0)],
            scale,
            source: [image.width, image.height],
        })
    }

    pub fn crop(&self) -> [u32; 4] {
        [
            self.origin[0] as u32,
            self.origin[1] as u32,
            (f64::from(self.width) / self.scale).round() as u32,
            (f64::from(self.height) / self.scale).round() as u32,
        ]
    }

    pub fn scale(&self) -> f64 {
        self.scale
    }

    fn to_canvas(&self, p: [f64; 2]) -> [f64; 2] {
        [
            (p[0] - self.origin[0]) * self.scale,
            (p[1] - self.origin[1]) * self.scale,
        ]
    }

    fn blend(&mut self, x: i64, y: i64, color: Rgb, alpha: f64) {
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return;
        }
        let i = (y as usize * self.width as usize + x as usize) * 3;
        for c in 0..3 {
            let old = f64::from(self.pixels[i + c]);
            self.pixels[i + c] = (old + (f64::from(color[c]) - old) * alpha).round() as u8;
        }
    }

    fn disc(&mut self, p: [f64; 2], radius: f64, color: Rgb) {
        let r = radius.max(0.5);
        for y in (p[1] - r).floor() as i64..=(p[1] + r).ceil() as i64 {
            for x in (p[0] - r).floor() as i64..=(p[0] + r).ceil() as i64 {
                if (x as f64 - p[0]).hypot(y as f64 - p[1]) <= r {
                    self.blend(x, y, color, 1.0);
                }
            }
        }
    }

    fn segment(&mut self, a: [f64; 2], b: [f64; 2], radius: f64, color: Rgb) {
        let steps = ((b[0] - a[0]).hypot(b[1] - a[1]) * 2.0).ceil().max(1.0) as usize;
        for i in 0..=steps {
            let t = i as f64 / steps as f64;
            self.disc(
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t],
                radius,
                color,
            );
        }
    }

    /// 原图坐标的折线，带暗色描边，在粉、黑、灰底上都看得清
    pub fn polyline(&mut self, points: &[[f64; 2]], color: Rgb, width: f64) {
        let points: Vec<_> = points.iter().map(|&p| self.to_canvas(p)).collect();
        for pass in [(width / 2.0 + 1.0, BLACK), (width / 2.0, color)] {
            for pair in points.windows(2) {
                self.segment(pair[0], pair[1], pass.0, pass.1);
            }
            if let [only] = points.as_slice() {
                self.disc(*only, pass.0 + 1.0, pass.1);
            }
        }
    }

    /// 原图坐标的网格：间距 `step` px 的细线，上边与左边标原图坐标
    pub fn grid(&mut self, step: u32) {
        let step = step.max(1);
        let [x0, y0, w, h] = self.crop();
        let first = |start: u32| start.div_ceil(step) * step;
        let xs: Vec<u32> = (first(x0)..=x0 + w).step_by(step as usize).collect();
        let ys: Vec<u32> = (first(y0)..=y0 + h).step_by(step as usize).collect();
        for &x in &xs {
            let cx = self.to_canvas([f64::from(x), 0.0])[0].round() as i64;
            for y in 0..i64::from(self.height) {
                self.blend(cx, y, WHITE, 0.35);
            }
        }
        for &y in &ys {
            let cy = self.to_canvas([0.0, f64::from(y)])[1].round() as i64;
            for x in 0..i64::from(self.width) {
                self.blend(x, cy, WHITE, 0.35);
            }
        }
        for &x in &xs {
            let cx = self.to_canvas([f64::from(x), 0.0])[0].round() as i64;
            self.label(cx + 2, 2, x);
        }
        for &y in &ys {
            let cy = self.to_canvas([0.0, f64::from(y)])[1].round() as i64;
            self.label(2, cy + 2, y);
        }
    }

    /// 网格间距：画布上不密于约 60 px 的最小整档
    pub fn auto_grid(&self) -> u32 {
        [10, 25, 50, 100, 200, 500, 1000]
            .into_iter()
            .find(|&s| f64::from(s) * self.scale >= 60.0)
            .unwrap_or(1000)
    }

    fn label(&mut self, x: i64, y: i64, value: u32) {
        let text = value.to_string();
        let (gw, gh) = (3 * GLYPH as i64, 5 * GLYPH as i64);
        let width = text.len() as i64 * (gw + GLYPH as i64) + GLYPH as i64;
        for yy in y - 1..y + gh + 1 {
            for xx in x - 1..x + width {
                self.blend(xx, yy, BLACK, 0.7);
            }
        }
        for (k, ch) in text.bytes().enumerate() {
            let bits = DIGITS[usize::from(ch - b'0')];
            let left = x + GLYPH as i64 + k as i64 * (gw + GLYPH as i64);
            for row in 0..5 {
                for col in 0..3 {
                    if bits >> (14 - (row * 3 + col)) & 1 == 1 {
                        for dy in 0..GLYPH as i64 {
                            for dx in 0..GLYPH as i64 {
                                self.blend(
                                    left + col as i64 * GLYPH as i64 + dx,
                                    y + row as i64 * GLYPH as i64 + dy,
                                    WHITE,
                                    1.0,
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    pub fn png(&self) -> Result<Vec<u8>, String> {
        let image = image::RgbImage::from_raw(self.width, self.height, self.pixels.clone())
            .ok_or("叠图尺寸无效")?;
        let mut out = std::io::Cursor::new(Vec::new());
        image
            .write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| format!("编码 PNG 失败：{e}"))?;
        Ok(out.into_inner())
    }

    pub fn source_size(&self) -> [u32; 2] {
        self.source
    }
}

fn clip(crop: Option<[u32; 4]>, width: u32, height: u32) -> Result<[u32; 4], String> {
    let [x, y, w, h] = crop.unwrap_or([0, 0, width, height]);
    let (x1, y1) = (
        x.saturating_add(w).min(width),
        y.saturating_add(h).min(height),
    );
    if x >= x1 || y >= y1 {
        return Err(format!(
            "裁剪框 [{x}, {y}, {w}, {h}] 不在 {width}×{height} 的图内"
        ));
    }
    Ok([x, y, x1 - x, y1 - y])
}

/// 画布第 `o` 个像素在原图上覆盖的范围（缩小时取均值，放大时取最近点）
fn footprint(o: u32, scale: f64, start: u32, end: u32) -> (u32, u32) {
    let a = start + (f64::from(o) / scale).floor() as u32;
    let b = start + (f64::from(o + 1) / scale).floor() as u32;
    let a = a.min(end - 1);
    (a, b.clamp(a + 1, end))
}

/// 外接框外扩 `margin` 后裁到图内（原图像素 [x, y, w, h]）
pub fn bounds(points: &[[f64; 2]], margin: f64, size: [u32; 2]) -> Option<[u32; 4]> {
    let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
    for p in points {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    if points.is_empty() {
        return None;
    }
    let x = (lo[0] - margin).floor().max(0.0) as u32;
    let y = (lo[1] - margin).floor().max(0.0) as u32;
    let x1 = ((hi[0] + margin).ceil() as u32).min(size[0]);
    let y1 = ((hi[1] + margin).ceil() as u32).min(size[1]);
    (x1 > x && y1 > y).then(|| [x, y, x1 - x, y1 - y])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_drawn_at_a_source_pixel_lands_at_that_pixel_of_the_cropped_and_scaled_canvas() {
        let image = FrameImage::new(400, 300, vec![0; 400 * 300]);
        // 裁出 [100, 50, 200, 100]，长边 400 → 放大 2 倍
        let mut canvas = Canvas::new(&image, Some([100, 50, 200, 100]), 400).unwrap();
        assert_eq!(canvas.crop(), [100, 50, 200, 100]);
        canvas.polyline(&[[150.0, 80.0]], YELLOW, 1.0);
        let at = |x: usize, y: usize| {
            let i = (y * canvas.width as usize + x) * 3;
            [canvas.pixels[i], canvas.pixels[i + 1], canvas.pixels[i + 2]]
        };
        assert_eq!(at(100, 60), YELLOW);
        assert_eq!(at(130, 60), [0, 0, 0]);
    }
}
