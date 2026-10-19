use crate::frame::FrameImage;

#[derive(Clone, Copy, Debug)]
pub enum Perturb {
    Noise(f64),
    Brightness(f64),
    Contrast(f64),
    Gamma(f64),
    Blur(f64),
    Motion(usize, f64),
    Affine { dx: f64, dy: f64, deg: f64, scale: f64 },
}

impl Perturb {
    /// 内容变换后原图上的点 p 移到哪里（只有几何扰动才移动）。
    pub fn moved(&self, p: [f64; 2], w: u32, h: u32) -> [f64; 2] {
        match *self {
            Perturb::Affine { dx, dy, deg, scale } => {
                let (cx, cy) = (w as f64 * 0.5, h as f64 * 0.5);
                let (s, c) = deg.to_radians().sin_cos();
                let (x, y) = (p[0] - cx, p[1] - cy);
                [cx + scale * (c * x - s * y) + dx, cy + scale * (s * x + c * y) + dy]
            }
            _ => p,
        }
    }

    pub fn apply(&self, img: &FrameImage, seed: u64) -> FrameImage {
        match *self {
            Perturb::Noise(sigma) => {
                let mut rng = Rng(seed ^ 0x9E37_79B9_7F4A_7C15);
                map(img, |v| v + sigma * rng.gauss())
            }
            Perturb::Brightness(delta) => map(img, |v| v + delta),
            Perturb::Contrast(gain) => {
                let mean = img.pixels.iter().map(|&v| v as f64).sum::<f64>() / img.pixels.len().max(1) as f64;
                map(img, |v| mean + (v - mean) * gain)
            }
            Perturb::Gamma(g) => map(img, |v| 255.0 * (v / 255.0).powf(g)),
            Perturb::Blur(sigma) => gaussian(img, sigma),
            Perturb::Motion(len, deg) => motion(img, len, deg),
            Perturb::Affine { .. } => {
                let (w, h) = (img.width, img.height);
                let (cx, cy) = (w as f64 * 0.5, h as f64 * 0.5);
                let Perturb::Affine { dx, dy, deg, scale } = *self else { unreachable!() };
                let (s, c) = (-deg).to_radians().sin_cos();
                let mut out = vec![0u8; img.pixels.len()];
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        let (u, v) = ((x as f64 - cx - dx) / scale, (y as f64 - cy - dy) / scale);
                        out[y * w as usize + x] = clamp(sample(img, cx + c * u - s * v, cy + s * u + c * v));
                    }
                }
                FrameImage::new(w, h, out)
            }
        }
    }
}

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn gauss(&mut self) -> f64 {
        let u = self.unit().max(1e-12);
        let v = self.unit();
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }
}

fn clamp(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn map(img: &FrameImage, mut f: impl FnMut(f64) -> f64) -> FrameImage {
    FrameImage::new(img.width, img.height, img.pixels.iter().map(|&v| clamp(f(v as f64))).collect())
}

/// 双线性取值，出界按边缘像素延伸（不引入假的暗边）。
pub fn sample(img: &FrameImage, x: f64, y: f64) -> f64 {
    let (w, h) = (img.width as i64, img.height as i64);
    let x = x.clamp(0.0, (w - 1) as f64);
    let y = y.clamp(0.0, (h - 1) as f64);
    let (x0, y0) = (x.floor() as i64, y.floor() as i64);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (x - x0 as f64, y - y0 as f64);
    let p = |x: i64, y: i64| img.pixels[(y * w + x) as usize] as f64;
    (p(x0, y0) * (1.0 - tx) + p(x1, y0) * tx) * (1.0 - ty) + (p(x0, y1) * (1.0 - tx) + p(x1, y1) * tx) * ty
}

fn gaussian(img: &FrameImage, sigma: f64) -> FrameImage {
    let r = (3.0 * sigma).ceil() as i64;
    let k: Vec<f64> = (-r..=r).map(|i| (-(i * i) as f64 / (2.0 * sigma * sigma)).exp()).collect();
    let sum: f64 = k.iter().sum();
    let (w, h) = (img.width as i64, img.height as i64);
    let mut tmp = vec![0f64; img.pixels.len()];
    for y in 0..h {
        for x in 0..w {
            tmp[(y * w + x) as usize] = (-r..=r).map(|i| k[(i + r) as usize] * img.pixels[(y * w + (x + i).clamp(0, w - 1)) as usize] as f64).sum::<f64>() / sum;
        }
    }
    let mut out = vec![0u8; img.pixels.len()];
    for y in 0..h {
        for x in 0..w {
            out[(y * w + x) as usize] = clamp((-r..=r).map(|i| k[(i + r) as usize] * tmp[((y + i).clamp(0, h - 1) * w + x) as usize]).sum::<f64>() / sum);
        }
    }
    FrameImage::new(img.width, img.height, out)
}

fn motion(img: &FrameImage, len: usize, deg: f64) -> FrameImage {
    let (s, c) = deg.to_radians().sin_cos();
    let n = len.max(1);
    let (w, h) = (img.width as usize, img.height as usize);
    let mut out = vec![0u8; img.pixels.len()];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for i in 0..n {
                let t = i as f64 - (n - 1) as f64 * 0.5;
                acc += sample(img, x as f64 + c * t, y as f64 + s * t);
            }
            out[y * w + x] = clamp(acc / n as f64);
        }
    }
    FrameImage::new(img.width, img.height, out)
}

/// 胶条沿示教线的局部坐标：每站中心 c、单位法向 n（与算法输出同向）、胶边 lo / hi（沿 n 的偏移，px）。
pub struct BeadFrame {
    pub c: Vec<[f64; 2]>,
    pub n: Vec<[f64; 2]>,
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
}

impl BeadFrame {
    /// edges 为空的站（基准测量无胶）按两侧有胶站线性插值胶边。
    pub fn new(centers: &[[f32; 2]], edges: &[Option<[[f32; 2]; 2]>]) -> Option<Self> {
        let k = centers.len();
        let c: Vec<[f64; 2]> = centers.iter().map(|p| [p[0] as f64, p[1] as f64]).collect();
        let mut n = vec![[0.0, 0.0]; k];
        for i in 0..k {
            let (a, b) = (c[i.saturating_sub(1)], c[(i + 1).min(k - 1)]);
            let t = [b[0] - a[0], b[1] - a[1]];
            let l = t[0].hypot(t[1]).max(1e-9);
            n[i] = [-t[1] / l, t[0] / l];
        }
        let flip = edges.iter().zip(&n).filter_map(|(e, n)| e.map(|e| ((e[1][0] - e[0][0]) as f64 * n[0] + (e[1][1] - e[0][1]) as f64 * n[1]).signum())).sum::<f64>() < 0.0;
        if flip {
            for v in &mut n { *v = [-v[0], -v[1]]; }
        }
        let mut lo = vec![f64::NAN; k];
        let mut hi = vec![f64::NAN; k];
        for i in 0..k {
            if let Some(e) = edges[i] {
                let d = |p: [f32; 2]| (p[0] as f64 - c[i][0]) * n[i][0] + (p[1] as f64 - c[i][1]) * n[i][1];
                let (a, b) = (d(e[0]), d(e[1]));
                lo[i] = a.min(b);
                hi[i] = a.max(b);
            }
        }
        let known: Vec<usize> = (0..k).filter(|&i| lo[i].is_finite()).collect();
        if known.is_empty() { return None; }
        for i in 0..k {
            if lo[i].is_finite() { continue; }
            let prev = known.iter().rev().find(|&&j| j < i).copied();
            let next = known.iter().find(|&&j| j > i).copied();
            let (a, b) = match (prev, next) { (Some(a), Some(b)) => (a, b), (Some(a), None) => (a, a), (None, Some(b)) => (b, b), _ => unreachable!() };
            let f = if b > a { (i - a) as f64 / (b - a) as f64 } else { 0.0 };
            lo[i] = lo[a] + (lo[b] - lo[a]) * f;
            hi[i] = hi[a] + (hi[b] - hi[a]) * f;
        }
        Some(Self { c, n, lo, hi })
    }

    fn at(&self, u: f64) -> ([f64; 2], [f64; 2], f64, f64) {
        let k = self.c.len();
        let i = (u.floor() as usize).min(k - 1);
        let j = (i + 1).min(k - 1);
        let f = (u - i as f64).clamp(0.0, 1.0);
        let lerp2 = |a: [f64; 2], b: [f64; 2]| [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f];
        let n = lerp2(self.n[i], self.n[j]);
        let l = n[0].hypot(n[1]).max(1e-9);
        (lerp2(self.c[i], self.c[j]), [n[0] / l, n[1] / l], self.lo[i] + (self.lo[j] - self.lo[i]) * f, self.hi[i] + (self.hi[j] - self.hi[i]) * f)
    }

    /// 像素在站序 [a, b] 里的局部坐标 (u, t)：u 是小数站序，t 是沿法向离中心的距离。
    fn locate(&self, p: [f64; 2], a: usize, b: usize) -> Option<(f64, f64)> {
        let mut best: Option<(f64, f64)> = None;
        for i in a..b.min(self.c.len() - 1) {
            let (p0, p1) = (self.c[i], self.c[i + 1]);
            let d = [p1[0] - p0[0], p1[1] - p0[1]];
            let l2 = (d[0] * d[0] + d[1] * d[1]).max(1e-12);
            let f = (((p[0] - p0[0]) * d[0] + (p[1] - p0[1]) * d[1]) / l2).clamp(0.0, 1.0);
            let q = [p0[0] + d[0] * f, p0[1] + d[1] * f];
            let dist = (p[0] - q[0]).hypot(p[1] - q[1]);
            if best.is_none_or(|b| dist < b.0) {
                best = Some((dist, i as f64 + f));
            }
        }
        let (_, u) = best?;
        let (c, n, _, _) = self.at(u);
        Some((u, (p[0] - c[0]) * n[0] + (p[1] - c[1]) * n[1]))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Defect {
    /// 断胶：整段胶抹成两侧背景
    Break,
    /// 胶宽乘以系数
    Width(f64),
    /// 胶条横向平移（px，沿法向）
    Offset(f64),
    /// 气泡：胶中心一个直径 = 系数 × 胶宽的亮洞
    Bubble(f64),
    /// 缺口：胶的一侧缺掉一半宽
    Notch,
}

const MARGIN: f64 = 3.0;

/// 在站序 [a, b]（含两端，小数）上合成缺陷；ramp 是宽度 / 偏移渐变的站数。
pub fn defect(img: &FrameImage, f: &BeadFrame, a: f64, b: f64, kind: Defect, ramp: f64) -> FrameImage {
    let k = f.c.len();
    let ia = (a.floor() as usize).saturating_sub(ramp.ceil() as usize + 2);
    let ib = ((b.ceil() as usize) + ramp.ceil() as usize + 2).min(k - 1);
    let reach = match kind {
        Defect::Width(g) => g.max(1.0),
        Defect::Offset(d) => 1.0 + d.abs() / 4.0,
        _ => 1.0,
    };
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for i in ia..=ib {
        let ext = (f.lo[i].abs().max(f.hi[i].abs()) + MARGIN) * reach + match kind { Defect::Offset(d) => d.abs(), _ => 0.0 } + 4.0;
        x0 = x0.min(f.c[i][0] - ext); x1 = x1.max(f.c[i][0] + ext);
        y0 = y0.min(f.c[i][1] - ext); y1 = y1.max(f.c[i][1] + ext);
    }
    let (w, h) = (img.width as i64, img.height as i64);
    let mut out = img.pixels.clone();
    let weight = |u: f64| -> f64 {
        if u < a - ramp || u > b + ramp { 0.0 }
        else if u < a { (u - (a - ramp)) / ramp.max(1e-9) }
        else if u > b { ((b + ramp) - u) / ramp.max(1e-9) }
        else { 1.0 }
    };
    for y in (y0.floor() as i64).max(0)..=(y1.ceil() as i64).min(h - 1) {
        for x in (x0.floor() as i64).max(0)..=(x1.ceil() as i64).min(w - 1) {
            let p = [x as f64, y as f64];
            let Some((u, t)) = f.locate(p, ia, ib) else { continue };
            if u <= ia as f64 + 1e-9 || u >= ib as f64 - 1e-9 { continue; }
            let (c, n, lo, hi) = f.at(u);
            let mid = 0.5 * (lo + hi);
            let half = 0.5 * (hi - lo) + MARGIN;
            let orig = |t: f64| sample(img, c[0] + n[0] * t, c[1] + n[1] * t);
            let outward = |s: f64, dir: f64| (0..4).map(|k| orig(s + dir * k as f64)).sum::<f64>() / 4.0;
            let (gl, gr) = (outward(mid - half, -1.0), outward(mid + half, 1.0));
            let bg = |t: f64| gl + (gr - gl) * ((t - (mid - half)) / (2.0 * half)).clamp(0.0, 1.0);
            let wgt = weight(u);
            let value = match kind {
                Defect::Break => {
                    if !(a..=b).contains(&u) || (t - mid).abs() > half { continue; }
                    bg(t)
                }
                Defect::Width(g) => {
                    let g = 1.0 + (g - 1.0) * wgt;
                    if wgt <= 0.0 || (t - mid).abs() > half * g.max(1.0) { continue; }
                    let src = mid + (t - mid) / g;
                    if (src - mid).abs() <= half { orig(src) } else { bg(t) }
                }
                Defect::Offset(d) => {
                    let d = d * wgt;
                    if wgt <= 0.0 || (t - mid).abs() > half + d.abs() { continue; }
                    let src = t - d;
                    if (src - mid).abs() <= half { orig(src) } else if (t - mid).abs() <= half { bg(t) } else { continue }
                }
                Defect::Bubble(g) => {
                    let center = 0.5 * (a + b);
                    let (cc, cn, clo, chi) = f.at(center);
                    let cm = 0.5 * (clo + chi);
                    let q = [cc[0] + cn[0] * cm, cc[1] + cn[1] * cm];
                    let r = 0.5 * g * (chi - clo);
                    let dist = (p[0] - q[0]).hypot(p[1] - q[1]);
                    if dist > r + 1.0 { continue; }
                    let blend = (r + 1.0 - dist).clamp(0.0, 1.0);
                    let cur = img.pixels[(y * w + x) as usize] as f64;
                    cur + (bg(t) - cur) * blend
                }
                Defect::Notch => {
                    if !(a..=b).contains(&u) || t < mid || t > mid + half { continue; }
                    bg(t)
                }
            };
            out[(y * w + x) as usize] = clamp(value);
        }
    }
    FrameImage::new(img.width, img.height, out)
}
