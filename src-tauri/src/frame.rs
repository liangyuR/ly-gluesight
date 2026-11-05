//! 相机帧与整帧像素缓冲池。海康相机每帧都新分配 1–5 MB 会让内存抖动，
//! 取图回调拷出来的缓冲用完（最后一个 Arc 释放）就回到池里给下一帧用。
//! 带预算的池（[`FramePool::with_budget`]）按字节封顶在途图像：检测积压时不随队列条数无限占内存（D-SB-32）。

use std::borrow::Cow;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;

/// 一帧 8 位图，行主序、无行填充：灰度每像素 1 字节，彩色每像素 3 字节（R、G、B 交错）。
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    /// 1 = 灰度，3 = RGB
    pub channels: u8,
    pub pixels: Vec<u8>,
    pool: Option<Weak<PoolInner>>,
    /// 记在池在途字节上的数目，释放时归还
    charged: usize,
}

impl FrameImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            channels: 1,
            pixels,
            pool: None,
            charged: 0,
        }
    }

    pub fn rgb(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self {
            width,
            height,
            channels: 3,
            pixels,
            pool: None,
            charged: 0,
        }
    }

    pub fn is_color(&self) -> bool {
        self.channels == 3
    }

    /// 宽 × 高 × 通道数；尺寸为 0 或溢出时为 None。
    pub fn expected_len(&self) -> Option<usize> {
        (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(self.channels as usize))
            .filter(|&n| n > 0)
    }

    /// 像素缓冲长度与宽、高、通道数相符。
    pub fn consistent(&self) -> bool {
        matches!(self.channels, 1 | 3) && self.expected_len() == Some(self.pixels.len())
    }

    /// 灰度像素：灰度图借用原像素，RGB 按 BT.601 整数近似 (77R + 150G + 29B + 128) >> 8 换算。
    pub fn luma(&self) -> Cow<'_, [u8]> {
        if self.channels != 3 {
            return Cow::Borrowed(&self.pixels);
        }
        Cow::Owned(
            self.pixels
                .chunks_exact(3)
                .map(|p| {
                    ((77 * p[0] as u32 + 150 * p[1] as u32 + 29 * p[2] as u32 + 128) >> 8) as u8
                })
                .collect(),
        )
    }

    /// 灰度副本（见 [`luma`](Self::luma)）。
    pub fn to_luma(&self) -> FrameImage {
        FrameImage::new(self.width, self.height, self.luma().into_owned())
    }
}

impl std::fmt::Debug for FrameImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "FrameImage({}×{}×{})",
            self.width, self.height, self.channels
        )
    }
}

/// 读图命令的原始字节（与前端约定）：前 20 字节依次是预览宽、高、原图宽、高、通道数（u32 小端），
/// 其后是最近邻缩到 900×650 以内的 预览宽 × 预览高 × 通道数 字节（RGB 交错）。
pub fn preview_bytes(image: &FrameImage) -> Vec<u8> {
    let channels = image.channels.max(1) as usize;
    let scale = (image.width as f32 / 900.0)
        .max(image.height as f32 / 650.0)
        .max(1.0);
    let w = (image.width as f32 / scale).max(1.0) as u32;
    let h = (image.height as f32 / scale).max(1.0) as u32;
    let mut bytes = Vec::with_capacity(20 + (w * h) as usize * channels);
    for n in [w, h, image.width, image.height, channels as u32] {
        bytes.extend(n.to_le_bytes());
    }
    for y in 0..h {
        for x in 0..w {
            let iy = ((y as f32 * scale) as u32).min(image.height - 1);
            let ix = ((x as f32 * scale) as u32).min(image.width - 1);
            let at = (iy * image.width + ix) as usize * channels;
            bytes.extend_from_slice(&image.pixels[at..at + channels]);
        }
    }
    bytes
}

impl Drop for FrameImage {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take().and_then(|p| p.upgrade()) {
            pool.in_flight.fetch_sub(self.charged, Ordering::SeqCst);
            pool.put(std::mem::take(&mut self.pixels));
        }
    }
}

struct PoolInner {
    free: Mutex<Vec<Vec<u8>>>,
    keep: usize,
    /// 在途图像字节上限；None 不限
    max_bytes: Option<usize>,
    /// 已交出、还没释放（最后一个 Arc 没放掉）的图像字节数；空闲缓冲不算
    in_flight: AtomicUsize,
}

impl PoolInner {
    fn put(&self, buf: Vec<u8>) {
        let mut free = self.free.lock().unwrap();
        if free.len() < self.keep {
            free.push(buf);
        }
    }

    fn reserve(&self, need: usize) -> Result<(), BudgetExceeded> {
        let Some(max) = self.max_bytes else {
            self.in_flight.fetch_add(need, Ordering::SeqCst);
            return Ok(());
        };
        self.in_flight.try_update(Ordering::SeqCst, Ordering::SeqCst, |used| used.checked_add(need).filter(|&n| n <= max))
            .map(|_| ())
            .map_err(|in_flight| BudgetExceeded { in_flight, need, max })
    }
}

/// 在途图像已到预算：这一帧不拷图（取图回调按缺帧处理，不中断回调）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BudgetExceeded {
    pub in_flight: usize,
    pub need: usize,
    pub max: usize,
}

impl std::fmt::Display for BudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mib = |n: usize| n as f64 / (1024.0 * 1024.0);
        write!(f, "图像内存预算已满：在途 {:.1} MiB + 本帧 {:.1} MiB 超过上限 {:.0} MiB", mib(self.in_flight), mib(self.need), mib(self.max))
    }
}

/// Bayer 转灰度失败：预算满（按缺帧处理）与输入不合法（图像不可用）分开报。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GrayError {
    BudgetExceeded(BudgetExceeded),
    Invalid(String),
}

#[derive(Clone)]
pub struct FramePool(Arc<PoolInner>);

impl FramePool {

    /// 在途图像最多 `max_bytes` 字节（每台相机一个池，见 camera::IMAGE_BUDGET_PER_CAMERA）；
    /// 超出时 [`copy`](Self::copy) / [`bayer8_to_gray`](Self::bayer8_to_gray) 返回 BudgetExceeded，
    /// 直到前面的图像释放（最后一个 Arc 放掉）。空闲缓冲（最多 `keep` 块）不计入。
    pub fn with_budget(keep: usize, max_bytes: usize) -> Self {
        Self::build(keep, Some(max_bytes))
    }

    fn build(keep: usize, max_bytes: Option<usize>) -> Self {
        Self(Arc::new(PoolInner { free: Mutex::new(Vec::new()), keep, max_bytes, in_flight: AtomicUsize::new(0) }))
    }

    /// 先记账再取缓冲：后续填充失败时图像随即释放，记的账一并归还。
    fn take(&self, width: u32, height: u32, bytes: usize) -> Result<FrameImage, BudgetExceeded> {
        self.0.reserve(bytes)?;
        let pixels = self.0.free.lock().unwrap().pop().unwrap_or_default();
        Ok(FrameImage {
            width,
            height,
            channels: 1,
            pixels,
            pool: Some(Arc::downgrade(&self.0)),
            charged: bytes,
        })
    }

    /// 拷一份像素到池里的缓冲。
    pub fn copy(&self, width: u32, height: u32, src: &[u8]) -> Result<FrameImage, BudgetExceeded> {
        let mut image = self.take(width, height, src.len())?;
        image.pixels.clear();
        image.pixels.extend_from_slice(src);
        Ok(image)
    }

    /// 8 位 Bayer 转成灰度写进池里的缓冲（见 [`bayer8_gray_into`]）。
    pub fn bayer8_to_gray(&self, width: u32, height: u32, src: &[u8]) -> Result<FrameImage, GrayError> {
        let bytes = (width as usize).saturating_mul(height as usize);
        let mut image = self.take(width, height, bytes).map_err(GrayError::BudgetExceeded)?;
        bayer8_gray_into(width as usize, height as usize, src, &mut image.pixels).map_err(GrayError::Invalid)?;
        Ok(image)
    }

    /// 8 位 Bayer 去马赛克成 RGB 写进池里的缓冲（见 [`bayer8_rgb_into`]），按 宽 × 高 × 3 记预算。
    pub fn bayer8_to_rgb(
        &self,
        pattern: BayerPattern,
        width: u32,
        height: u32,
        src: &[u8],
    ) -> Result<FrameImage, GrayError> {
        let bytes = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(3);
        let mut image = self
            .take(width, height, bytes)
            .map_err(GrayError::BudgetExceeded)?;
        image.channels = 3;
        bayer8_rgb_into(
            pattern,
            width as usize,
            height as usize,
            src,
            &mut image.pixels,
        )
        .map_err(GrayError::Invalid)?;
        Ok(image)
    }
}

/// 8 位 Bayer 马赛克双线性去马赛克成 RGB（交错、行紧排）：本像素的颜色取原值，缺的两色取 3×3 邻域里同色像素的均值
/// （四舍五入）——内部即标准双线性（G 取上下左右、R/B 取对角或同行同列的两个），边界只用图内的邻居。
pub fn bayer8_rgb_into(
    pattern: BayerPattern,
    w: usize,
    h: usize,
    src: &[u8],
    out: &mut Vec<u8>,
) -> Result<(), String> {
    if w < 2 || h < 2 {
        return Err(format!("Bayer 图像至少 2×2（实际 {w}×{h}）"));
    }
    let n = w * h;
    if src.len() < n {
        return Err(format!(
            "Bayer 像素数据不足：{w}×{h} 需要 {n} 字节，实际 {}",
            src.len()
        ));
    }
    let sites = pattern.sites();
    out.clear();
    out.resize(n * 3, 0);
    let border = |out: &mut [u8], x: usize, y: usize| {
        let (y0, y1) = (y.saturating_sub(1), (y + 1).min(h - 1));
        let (x0, x1) = (x.saturating_sub(1), (x + 1).min(w - 1));
        let (mut sum, mut count) = ([0u32; 3], [0u32; 3]);
        for ny in y0..=y1 {
            let row = &src[ny * w..(ny + 1) * w];
            let phase = (ny & 1) * 2;
            for (nx, &v) in row.iter().enumerate().take(x1 + 1).skip(x0) {
                let c = sites[phase + (nx & 1)] as usize;
                sum[c] += v as u32;
                count[c] += 1;
            }
        }
        let own = sites[(y & 1) * 2 + (x & 1)] as usize;
        let o = (y * w + x) * 3;
        for c in 0..3 {
            out[o + c] = if c == own {
                src[y * w + x]
            } else {
                ((sum[c] + count[c] / 2) / count[c]) as u8
            };
        }
    };
    for y in 0..h {
        if y == 0 || y == h - 1 {
            (0..w).for_each(|x| border(out, x, y));
            continue;
        }
        border(out, 0, y);
        border(out, w - 1, y);
        // 内部像素的 3×3 邻域是完整的：G 位取左右、上下各两个异色的均值，R/B 位取十字四个 G 与对角四个另一色的均值
        let (up, row, down) = (
            &src[(y - 1) * w..y * w],
            &src[y * w..(y + 1) * w],
            &src[(y + 1) * w..(y + 2) * w],
        );
        let phase = (y & 1) * 2;
        for x in 1..w - 1 {
            let own = sites[phase + (x & 1)] as usize;
            let o = (y * w + x) * 3;
            let px = &mut out[o..o + 3];
            px[own] = row[x];
            if own == 1 {
                let side = sites[phase + ((x + 1) & 1)] as usize;
                let vert = sites[((y + 1) & 1) * 2 + (x & 1)] as usize;
                px[side] = ((row[x - 1] as u32 + row[x + 1] as u32 + 1) / 2) as u8;
                px[vert] = ((up[x] as u32 + down[x] as u32 + 1) / 2) as u8;
            } else {
                let cross = up[x] as u32 + down[x] as u32 + row[x - 1] as u32 + row[x + 1] as u32;
                let diag =
                    up[x - 1] as u32 + up[x + 1] as u32 + down[x - 1] as u32 + down[x + 1] as u32;
                px[1] = ((cross + 2) / 4) as u8;
                px[2 - own] = ((diag + 2) / 4) as u8;
            }
        }
    }
    Ok(())
}

/// 8 位 Bayer 马赛克（任一相位）转全分辨率灰度（D-9）：像素 (x, y) 取左上角为
/// (min(x, w-2), min(y, h-2)) 的 2×2 块，输出 (四个之和 + 2) / 4。任一 2×2 块都是一 R 两 G 一 B，
/// 所以等于 (R + 2G + B) / 4，与相位无关；末行末列往回退一格，块仍是完整的 Bayer 四格。
pub fn bayer8_gray_into(w: usize, h: usize, src: &[u8], out: &mut Vec<u8>) -> Result<(), String> {
    if w < 2 || h < 2 {
        return Err(format!("Bayer 图像至少 2×2（实际 {w}×{h}）"));
    }
    let n = w * h;
    if src.len() < n {
        return Err(format!("Bayer 像素数据不足：{w}×{h} 需要 {n} 字节，实际 {}", src.len()));
    }
    out.clear();
    out.reserve(n);
    for by in 0..h - 1 {
        let r0 = &src[by * w..(by + 1) * w];
        let r1 = &src[(by + 1) * w..(by + 2) * w];
        out.extend(r0.iter().zip(&r0[1..]).zip(r1.iter().zip(&r1[1..])).map(|((&a, &b), (&c, &d))| {
            ((a as u16 + b as u16 + c as u16 + d as u16 + 2) >> 2) as u8
        }));
        // 末列与倒数第二列同块
        out.push(out[out.len() - 1]);
    }
    // 末行与倒数第二行同块
    out.extend_from_within((h - 2) * w..(h - 1) * w);
    Ok(())
}

/// 8 位 Bayer 马赛克的相位，按 GenICam 命名（第 0 行前两个像素、第 1 行前两个像素）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BayerPattern {
    Bg,
    Rg,
    Gb,
    Gr,
}

impl BayerPattern {
    pub fn name(self) -> &'static str {
        match self {
            BayerPattern::Bg => "BG",
            BayerPattern::Rg => "RG",
            BayerPattern::Gb => "GB",
            BayerPattern::Gr => "GR",
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        [
            BayerPattern::Bg,
            BayerPattern::Rg,
            BayerPattern::Gb,
            BayerPattern::Gr,
        ]
        .into_iter()
        .find(|p| p.name().eq_ignore_ascii_case(name))
    }

    /// 2×2 相位里各格的颜色（0 = R、1 = G、2 = B），按 (行 & 1) × 2 + (列 & 1) 取。
    fn sites(self) -> [u8; 4] {
        match self {
            BayerPattern::Bg => [2, 1, 1, 0],
            BayerPattern::Rg => [0, 1, 1, 2],
            BayerPattern::Gb => [1, 2, 0, 1],
            BayerPattern::Gr => [1, 0, 2, 1],
        }
    }
}

/// 相机原样输出的 8 位 Bayer 马赛克：彩色模式下每帧都带，灰度模式下检测设置“同时保存原始 Bayer”打开时带上；只录制、不参与测量。
#[derive(Clone, Debug)]
pub struct BayerMosaic {
    pub pattern: BayerPattern,
    pub image: Arc<FrameImage>,
}

/// 帧的触发计数从哪来。只有触发计数能把帧认到拍照点上：帧计数、SDK 帧号只数相机发出或主机收到的帧，
/// 丢帧、过触发时与 PLC 发的触发对不上。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CounterSource {
    /// 海康 Chunk 触发计数（`nTriggerIndex`）
    ChunkTrigger,
    /// 海康只有 Chunk 帧计数，没有触发计数
    ChunkFrame,
    /// 没有 Chunk，只有 SDK 帧号
    SdkFrame,
    /// 模拟、样本集相机自己编的号：每次触发加一，（重新）加载时从 0 重来
    Synthetic,
}

impl CounterSource {
    pub fn label(self) -> &'static str {
        match self {
            CounterSource::ChunkTrigger => "Chunk 触发计数",
            CounterSource::ChunkFrame => "Chunk 帧计数",
            CounterSource::SdkFrame => "SDK 帧号",
            CounterSource::Synthetic => "模拟计数",
        }
    }
}

/// 一帧图像的元数据。帧计数取自 Chunk 帧计数，没有时为 SDK 帧号；触发计数只在来源（`counter`）是
/// Chunk 触发计数或模拟时是真的触发计数，其他来源时数值照填帧计数，只给界面和日志看。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// 相机组里的序号（从 0 开始）
    pub cam: u8,
    /// 设备会话号：海康每打开一次、模拟 / 样本集每（重新）加载一次换一个新号，进程内不重复。
    /// 会话变了，触发计数可能从头数起
    pub session: u64,
    /// 触发计数的来源
    pub counter: CounterSource,
    pub frame_counter: u64,
    pub trigger_counter: u64,
    pub lost_packets: u32,
    pub ts: i64,
    /// 相机时间戳（ns，D-SB-55）：海康取自 `nDevTimeStamp` 按 `GevTimestampTickFrequency` 换算，重开相机后仍连续；
    /// 模拟、样本集为合成时钟（帧计数 × `frame_clock::SYNTH_PERIOD_NS`）。读不到时为 None
    pub dev_ts: Option<u64>,
    /// 软触发（示教取图、样本集"下一张"）出来的帧
    #[serde(skip)]
    pub manual: bool,
    /// 按视角 1–3 排列的整帧图（灰度或 RGB）；三目必须整组交付。单视角模拟测量可只交元数据。
    #[serde(skip)]
    pub images: Vec<Arc<FrameImage>>,
    /// 单视角彩色相机的原始马赛克：彩色模式每帧都带，灰度模式只在保存原始 Bayer 时带上
    #[serde(skip)]
    pub bayer: Option<BayerMosaic>,
}

impl Frame {
    /// 单视角独立相机的那幅图（段模型用，D-SB-11）：恰好一幅时才有；只有元数据或三目拼接的帧没有。
    pub fn single_image(&self) -> Option<&Arc<FrameImage>> {
        match self.images.as_slice() {
            [image] => Some(image),
            _ => None,
        }
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_pool_over_budget_refuses_allocation_until_frames_released() {
        type Alloc = fn(&FramePool) -> Result<FrameImage, BudgetExceeded>;
        let cases: [(&str, Alloc); 2] = [
            ("拷贝", |pool| pool.copy(4, 4, &[7; 16])),
            ("Bayer 转灰度", |pool| pool.bayer8_to_gray(4, 4, &[7; 16]).map_err(|e| match e {
                GrayError::BudgetExceeded(full) => full,
                GrayError::Invalid(e) => panic!("{e}"),
            })),
        ];
        for (name, alloc) in cases {
            // 预算正好两幅 4×4
            let pool = FramePool::with_budget(4, 32);
            let first = Arc::new(alloc(&pool).unwrap());
            let held_elsewhere = first.clone();
            let _second = alloc(&pool).unwrap();
            assert_eq!(alloc(&pool).err(), Some(BudgetExceeded { in_flight: 32, need: 16, max: 32 }), "{name}");
            drop(first);
            assert!(alloc(&pool).is_err(), "{name}：别处还持有这幅图，预算不能归还");
            drop(held_elsewhere);
            assert!(alloc(&pool).is_ok(), "{name}：最后一个引用释放后预算归还");
        }
    }

    #[test]
    fn demosaic_of_each_genicam_phase_keeps_red_and_blue_in_place() {
        // 防的回归：GenICam 相位（BayerBG = 第 0 行 B G、第 1 行 G R）与 OpenCV 命名差一格，按错的相位去马赛克红蓝互换，
        // 粉色胶条变成紫色
        let pink = [230u8, 120, 130];
        let (w, h) = (6usize, 5usize);
        for pattern in [
            BayerPattern::Bg,
            BayerPattern::Rg,
            BayerPattern::Gb,
            BayerPattern::Gr,
        ] {
            let row0: Vec<char> = pattern.name().chars().collect();
            let row1: Vec<char> = row0
                .iter()
                .map(|&c| match c {
                    'G' => {
                        if row0.contains(&'R') {
                            'B'
                        } else {
                            'R'
                        }
                    }
                    _ => 'G',
                })
                .collect();
            let mosaic: Vec<u8> = (0..h)
                .flat_map(|y| (0..w).map(move |x| (y, x)))
                .map(|(y, x)| {
                    let color = if y % 2 == 0 { row0[x % 2] } else { row1[x % 2] };
                    pink["RGB".find(color).unwrap()]
                })
                .collect();
            let mut rgb = Vec::new();
            bayer8_rgb_into(pattern, w, h, &mosaic, &mut rgb).unwrap();
            assert_eq!(rgb.len(), w * h * 3);
            for (i, px) in rgb.chunks_exact(3).enumerate() {
                for c in 0..3 {
                    assert!(
                        px[c].abs_diff(pink[c]) <= 2,
                        "{} 第 {i} 像素 {px:?}",
                        pattern.name()
                    );
                }
            }
        }
    }
}
