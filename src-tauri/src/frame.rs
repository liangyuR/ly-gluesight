//! 相机帧与整帧像素缓冲池。海康相机每帧都新分配 1–5 MB 会让内存抖动，
//! 取图回调拷出来的缓冲用完（最后一个 Arc 释放）就回到池里给下一帧用。
//! 带预算的池（[`FramePool::with_budget`]）按字节封顶在途图像：检测积压时不随队列条数无限占内存（D-SB-32）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;

/// 一帧 8 位灰度图，行主序、无行填充。
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pool: Option<Weak<PoolInner>>,
    /// 记在池在途字节上的数目，释放时归还
    charged: usize,
}

impl FrameImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self { width, height, pixels, pool: None, charged: 0 }
    }
}

impl std::fmt::Debug for FrameImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FrameImage({}×{})", self.width, self.height)
    }
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
        Ok(FrameImage { width, height, pixels, pool: Some(Arc::downgrade(&self.0)), charged: bytes })
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
    /// 软触发（示教取图、样本集"下一张"）出来的帧
    #[serde(skip)]
    pub manual: bool,
    /// 按视角 1–3 排列的整帧灰度图；三目必须整组交付。单视角模拟测量可只交元数据。
    #[serde(skip)]
    pub images: Vec<Arc<FrameImage>>,
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
}
