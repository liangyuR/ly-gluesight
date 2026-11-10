//! 相机帧与整帧像素缓冲池。海康相机每帧都新分配 1–5 MB 会让内存抖动，
//! 取图回调拷出来的缓冲用完（最后一个 Arc 释放）就回到池里给下一帧用。
//! 带预算的池（[`FramePool::with_budget`]）按字节封顶在途图像：检测积压时不随队列条数无限占内存（D-SB-32）。
//! 彩色帧在途只有原始马赛克（[`BayerMosaic`]），RGB 在要用时才去马赛克，使用期间各方共享同一份（IS-5）。

use std::borrow::Cow;
use std::collections::VecDeque;
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
    /// 由在途帧按需生成的图（记在池的 `derived` 上）
    derived: bool,
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
            derived: false,
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
            derived: false,
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
            pool.counter(self.derived)
                .fetch_sub(self.charged, Ordering::SeqCst);
            pool.put(std::mem::take(&mut self.pixels));
        }
    }
}

/// 每个池保留的最近几份按需生成的 RGB：测量、预览、读图先后要同一帧时不必重算；更早的随使用方放手而释放。
pub const RGB_KEEP: usize = 2;

struct PoolInner {
    /// 空闲缓冲：马赛克与 RGB 大小不同，按容量分开留，每种最多 `keep` 块
    free: Mutex<Vec<Vec<u8>>>,
    keep: usize,
    /// 在途图像字节上限；None 不限
    max_bytes: Option<usize>,
    /// 已交出、还没释放（最后一个 Arc 没放掉）的图像字节数；空闲缓冲不算
    in_flight: AtomicUsize,
    /// 由在途帧按需生成、还没释放的图（RGB、缩略图）字节数
    derived: AtomicUsize,
    /// 最近生成的 RGB（最多 [`RGB_KEEP`] 份）
    recent_rgb: Mutex<VecDeque<Arc<FrameImage>>>,
}

impl PoolInner {
    fn put(&self, buf: Vec<u8>) {
        let mut free = self.free.lock().unwrap();
        let same = free
            .iter()
            .filter(|b| b.capacity() == buf.capacity())
            .count();
        if same < self.keep && free.len() < self.keep * 2 {
            free.push(buf);
        }
    }

    /// 容量正好 `bytes` 的空闲缓冲，没有就新分配：马赛克不占用 RGB 大小的缓冲。
    fn buffer(&self, bytes: usize) -> Vec<u8> {
        let mut free = self.free.lock().unwrap();
        match free.iter().position(|b| b.capacity() == bytes) {
            Some(at) => free.swap_remove(at),
            None => Vec::with_capacity(bytes),
        }
    }

    fn counter(&self, derived: bool) -> &AtomicUsize {
        if derived {
            &self.derived
        } else {
            &self.in_flight
        }
    }

    /// 预算里给按需生成的图留出 `headroom` 字节，最多留预算的一半（大图的余量不能把原始帧的额度占光）；
    /// 它们实际占的超过留出的数时按实际占的扣。
    fn reserve(&self, need: usize, headroom: usize) -> Result<(), BudgetExceeded> {
        let headroom = self.max_bytes.map_or(headroom, |max| headroom.min(max / 2));
        let derived = self.derived.load(Ordering::SeqCst).max(headroom);
        let Some(max) = self.max_bytes.map(|max| max.saturating_sub(derived)) else {
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
        Self(Arc::new(PoolInner {
            free: Mutex::new(Vec::new()),
            keep,
            max_bytes,
            in_flight: AtomicUsize::new(0),
            derived: AtomicUsize::new(0),
            recent_rgb: Mutex::new(VecDeque::new()),
        }))
    }

    /// 先记账再取缓冲：后续填充失败时图像随即释放，记的账一并归还。
    fn take(
        &self,
        width: u32,
        height: u32,
        bytes: usize,
        headroom: usize,
    ) -> Result<FrameImage, BudgetExceeded> {
        self.0.reserve(bytes, headroom)?;
        Ok(FrameImage {
            width,
            height,
            channels: 1,
            pixels: self.0.buffer(bytes),
            pool: Some(Arc::downgrade(&self.0)),
            charged: bytes,
            derived: false,
        })
    }

    /// 拷一份像素到池里的缓冲。
    pub fn copy(&self, width: u32, height: u32, src: &[u8]) -> Result<FrameImage, BudgetExceeded> {
        self.copy_within(width, height, src, 0)
    }

    /// 拷一份像素到池里的缓冲，预算里留出 `headroom` 字节不用：取图回调收彩色帧的原始马赛克时，
    /// 给之后按需生成的 RGB 留地方。
    pub fn copy_within(
        &self,
        width: u32,
        height: u32,
        src: &[u8],
        headroom: usize,
    ) -> Result<FrameImage, BudgetExceeded> {
        let mut image = self.take(width, height, src.len(), headroom)?;
        image.pixels.clear();
        image.pixels.extend_from_slice(src);
        Ok(image)
    }

    /// 8 位 Bayer 转成灰度写进池里的缓冲（见 [`bayer8_gray_into`]）。
    pub fn bayer8_to_gray(&self, width: u32, height: u32, src: &[u8]) -> Result<FrameImage, GrayError> {
        let bytes = (width as usize).saturating_mul(height as usize);
        let mut image = self
            .take(width, height, bytes, 0)
            .map_err(GrayError::BudgetExceeded)?;
        bayer8_gray_into(width as usize, height as usize, src, &mut image.pixels).map_err(GrayError::Invalid)?;
        Ok(image)
    }

    /// 给由在途帧按需生成的图（RGB、缩略图）记账：记进同一预算但不拒绝——拒绝会让已收下的帧测不了、
    /// 积压的原始帧永远放不掉。这类图的份数由使用方封顶（测量线程数、[`RGB_KEEP`]、取图与读图），
    /// 回调收原始帧时已为它们留出余量（[`copy_within`](Self::copy_within)），超出余量的部分由回调少收新帧补回。
    pub fn charge(&self, bytes: usize) -> PoolCharge {
        self.0.derived.fetch_add(bytes, Ordering::SeqCst);
        PoolCharge {
            pool: Arc::downgrade(&self.0),
            bytes,
        }
    }
}

/// 记在池预算上的一笔字节数，放掉时归还。
pub struct PoolCharge {
    pool: Weak<PoolInner>,
    bytes: usize,
}

impl Drop for PoolCharge {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.upgrade() {
            pool.derived.fetch_sub(self.bytes, Ordering::SeqCst);
        }
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

/// 相机原样输出的 8 位 Bayer 马赛克：彩色模式下每帧在途的就是它，也是录制原图。测量、预览、读图用的 RGB
/// 由 [`rgb`](Self::rgb) 按需去马赛克，同一帧的各份克隆共享一个结果。
#[derive(Clone, Debug)]
pub struct BayerMosaic {
    pub pattern: BayerPattern,
    pub image: Arc<FrameImage>,
    rgb: Arc<Mutex<Weak<FrameImage>>>,
}

impl BayerMosaic {
    pub fn new(pattern: BayerPattern, image: Arc<FrameImage>) -> Self {
        Self {
            pattern,
            image,
            rgb: Arc::default(),
        }
    }

    /// 这一帧的 RGB（双线性去马赛克，见 [`bayer8_rgb_into`]）。还有人拿着时直接共享那一份，否则在调用线程里算一次；
    /// 同时要的各方等同一次计算。马赛克来自带预算的池时，RGB 记在同一个池上（不拒绝，见 [`FramePool::charge`]），
    /// 池另留最近 [`RGB_KEEP`] 份，更早的随最后一个使用方放手而释放。
    pub fn rgb(&self) -> Result<Arc<FrameImage>, String> {
        let mut shared = self.rgb.lock().unwrap();
        if let Some(rgb) = shared.upgrade() {
            return Ok(rgb);
        }
        let (width, height) = (self.image.width, self.image.height);
        let bytes = (width as usize)
            .saturating_mul(height as usize)
            .saturating_mul(3);
        let pool = self.image.pool.as_ref().and_then(Weak::upgrade);
        let mut rgb = match &pool {
            Some(pool) => {
                pool.derived.fetch_add(bytes, Ordering::SeqCst);
                FrameImage {
                    width,
                    height,
                    channels: 3,
                    pixels: pool.buffer(bytes),
                    pool: Some(Arc::downgrade(pool)),
                    charged: bytes,
                    derived: true,
                }
            }
            None => FrameImage::rgb(width, height, Vec::new()),
        };
        bayer8_rgb_into(
            self.pattern,
            width as usize,
            height as usize,
            &self.image.pixels,
            &mut rgb.pixels,
        )?;
        let rgb = Arc::new(rgb);
        *shared = Arc::downgrade(&rgb);
        if let Some(pool) = pool {
            let mut recent = pool.recent_rgb.lock().unwrap();
            recent.push_back(rgb.clone());
            while recent.len() > RGB_KEEP {
                recent.pop_front();
            }
        }
        Ok(rgb)
    }
}

/// 一帧交给测量、预览的图：现成的图（灰度，或离线来源给的 RGB），或要用时才去马赛克的彩色帧。
#[derive(Clone, Debug)]
pub enum FrameView {
    Image(Arc<FrameImage>),
    Mosaic(BayerMosaic),
}

impl FrameView {
    /// 测量、显示用的图；彩色帧在这里去马赛克（[`BayerMosaic::rgb`]）。
    pub fn image(&self) -> Result<Arc<FrameImage>, String> {
        match self {
            FrameView::Image(image) => Ok(image.clone()),
            FrameView::Mosaic(mosaic) => mosaic.rgb(),
        }
    }

    /// 留着这一帧占的字节数：现成的图按图算，彩色帧按原始马赛克算。
    pub fn held_bytes(&self) -> usize {
        match self {
            FrameView::Image(image) => image.pixels.len(),
            FrameView::Mosaic(mosaic) => mosaic.image.pixels.len(),
        }
    }
}

impl From<Arc<FrameImage>> for FrameView {
    fn from(image: Arc<FrameImage>) -> Self {
        FrameView::Image(image)
    }
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
    /// 按视角 1–3 排列的整帧图（灰度，或离线来源给的 RGB）；三目必须整组交付。单视角模拟测量可只交元数据。
    /// 海康彩色帧这里为空，图在 `bayer` 里
    #[serde(skip)]
    pub images: Vec<Arc<FrameImage>>,
    /// 海康单视角彩色相机的原始马赛克：彩色模式留了整帧的每帧都带，灰度模式没有
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

    /// 单视角独立相机这一帧的图：彩色帧是原始马赛克（要用时去马赛克），其余是现成的那幅；没留整帧时没有。
    pub fn view(&self) -> Option<FrameView> {
        match &self.bayer {
            Some(mosaic) => Some(FrameView::Mosaic(mosaic.clone())),
            None => self.single_image().cloned().map(FrameView::Image),
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

    /// 按文档逐像素算的去马赛克：本像素的颜色取原值，缺的两色取 3×3 邻域（图内部分）同色像素的均值，四舍五入。
    fn reference_rgb(layout: [usize; 4], w: usize, h: usize, src: &[u8]) -> Vec<u8> {
        let color = |x: usize, y: usize| layout[(y & 1) * 2 + (x & 1)];
        let mut out = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                for c in 0..3 {
                    if color(x, y) == c {
                        out.push(src[y * w + x]);
                        continue;
                    }
                    let (mut sum, mut count) = (0u32, 0u32);
                    for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                        for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                            if color(nx, ny) == c {
                                sum += u32::from(src[ny * w + nx]);
                                count += 1;
                            }
                        }
                    }
                    out.push(((sum + count / 2) / count) as u8);
                }
            }
        }
        out
    }

    /// 保护的规则：彩色帧改为回调只收原始马赛克、测量线程按需去马赛克后，交给测量引擎的 RGB 仍是
    /// 相机马赛克按相位双线性去马赛克的逐字节结果（IS-5：算法输入不变）。
    #[test]
    fn color_frame_view_gives_the_bilinear_demosaic_of_the_callback_bytes() {
        // 0 = R、1 = G、2 = B，按 GenICam 相位名排 2×2
        let layouts = [
            (BayerPattern::Bg, [2, 1, 1, 0]),
            (BayerPattern::Rg, [0, 1, 1, 2]),
            (BayerPattern::Gb, [1, 2, 0, 1]),
            (BayerPattern::Gr, [1, 0, 2, 1]),
        ];
        for (pattern, layout) in layouts {
            for (w, h) in [(7usize, 6usize), (2, 2), (6, 5)] {
                let sdk: Vec<u8> = (0..w * h)
                    .map(|i| ((i * 2_654_435_761) >> 7) as u8)
                    .collect();
                let pool = FramePool::with_budget(4, 1 << 20);
                let raw = pool.copy_within(w as u32, h as u32, &sdk, 0).unwrap();
                let frame = Frame {
                    cam: 0,
                    session: 1,
                    counter: CounterSource::SdkFrame,
                    frame_counter: 1,
                    trigger_counter: 1,
                    lost_packets: 0,
                    ts: 0,
                    dev_ts: None,
                    manual: false,
                    images: Vec::new(),
                    bayer: Some(BayerMosaic::new(pattern, Arc::new(raw))),
                };
                let image = frame.view().unwrap().image().unwrap();
                assert_eq!(
                    (image.width, image.height, image.channels),
                    (w as u32, h as u32, 3)
                );
                assert_eq!(
                    image.pixels,
                    reference_rgb(layout, w, h, &sdk),
                    "{} {w}×{h}",
                    pattern.name()
                );
            }
        }
    }

    /// 保护的规则：同一帧的 RGB 各使用方共享同一份、不各自去马赛克；池只留最近 RGB_KEEP 份，
    /// 更早的随使用方放手而释放，不随在途帧数增长。
    #[test]
    fn frame_rgb_is_shared_between_consumers_and_only_recent_ones_stay_alive() {
        let pool = FramePool::with_budget(4, 1 << 20);
        let frames: Vec<BayerMosaic> = (0..RGB_KEEP + 2)
            .map(|i| {
                let raw = pool.copy(4, 4, &[i as u8; 16]).unwrap();
                BayerMosaic::new(BayerPattern::Bg, Arc::new(raw))
            })
            .collect();
        let measured = frames[0].rgb().unwrap();
        let previewed = frames[0].clone().rgb().unwrap();
        assert!(Arc::ptr_eq(&measured, &previewed));
        drop((measured, previewed));
        let generated: Vec<Weak<FrameImage>> = frames
            .iter()
            .map(|frame| Arc::downgrade(&frame.rgb().unwrap()))
            .collect();
        let alive: Vec<bool> = generated
            .iter()
            .map(|rgb| rgb.upgrade().is_some())
            .collect();
        assert_eq!(alive, [false, false, true, true]);
    }

    /// 保护的规则：给 RGB 留的余量最多占预算的一半。防的回归是大图（如 4096×3072，8 份 RGB 超过整个 256 MiB 预算）
    /// 的余量把原始帧的额度扣到零，池空着也拒收每一帧，相机没有预览、生产帧全部按预算不足丢弃。
    #[test]
    fn headroom_larger_than_the_budget_still_leaves_half_for_raw_frames() {
        let pool = FramePool::with_budget(4, 4 * 16);
        let first = pool.copy_within(4, 4, &[1; 16], 1000).unwrap();
        let _second = pool.copy_within(4, 4, &[2; 16], 1000).unwrap();
        assert!(pool.copy_within(4, 4, &[3; 16], 1000).is_err());
        drop(first);
        assert!(pool.copy_within(4, 4, &[3; 16], 1000).is_ok());
    }

    /// 保护的规则：预算满 → 回调拒收新帧（记缺帧），但已收下的帧仍能生成 RGB 去测量；
    /// 按需生成的图在留出的余量之内时不挤占原始帧的额度。
    #[test]
    fn raw_frames_stop_at_the_budget_headroom_while_received_frames_still_get_rgb() {
        // 一帧 4×4：原始 16 字节、RGB 48 字节；预算放 4 帧原始 + 1 份 RGB
        let pool = FramePool::with_budget(4, 4 * 16 + 48);
        let receive = || {
            pool.copy_within(4, 4, &[9; 16], 48)
                .map(|raw| BayerMosaic::new(BayerPattern::Bg, Arc::new(raw)))
        };
        let mut backlog: Vec<BayerMosaic> = (0..4).map(|_| receive().unwrap()).collect();
        let full = BudgetExceeded {
            in_flight: 64,
            need: 16,
            max: 64,
        };
        assert_eq!(receive().err(), Some(full));
        let rgb = backlog[0].rgb().unwrap();
        assert_eq!(rgb.pixels.len(), 48);
        assert_eq!(receive().err(), Some(full), "RGB 不让出原始帧的额度");
        backlog.pop();
        assert!(receive().is_ok(), "一帧放掉后回调又能收下一帧");
    }
}
