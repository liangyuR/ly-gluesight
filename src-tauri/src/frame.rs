//! 相机帧与整帧像素缓冲池。海康相机每帧都新分配 1–5 MB 会让内存抖动，
//! 取图回调拷出来的缓冲用完（最后一个 Arc 释放）就回到池里给下一帧用。

use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;

/// 一帧 8 位灰度图，行主序、无行填充。
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    pool: Option<Weak<PoolInner>>,
}

impl FrameImage {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        Self { width, height, pixels, pool: None }
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
            pool.put(std::mem::take(&mut self.pixels));
        }
    }
}

struct PoolInner {
    free: Mutex<Vec<Vec<u8>>>,
    keep: usize,
}

impl PoolInner {
    fn put(&self, buf: Vec<u8>) {
        let mut free = self.free.lock().unwrap();
        if free.len() < self.keep {
            free.push(buf);
        }
    }
}

#[derive(Clone)]
pub struct FramePool(Arc<PoolInner>);

impl FramePool {
    /// `keep`：池里最多留几块空闲缓冲。
    pub fn new(keep: usize) -> Self {
        Self(Arc::new(PoolInner { free: Mutex::new(Vec::new()), keep }))
    }

    /// 拷一份像素到池里的缓冲。
    pub fn copy(&self, width: u32, height: u32, src: &[u8]) -> FrameImage {
        let mut buf = self.0.free.lock().unwrap().pop().unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(src);
        FrameImage { width, height, pixels: buf, pool: Some(Arc::downgrade(&self.0)) }
    }
}

/// 一帧图像的元数据。计数器取自相机 Chunk（帧计数、Line0 触发计数），未开启 Chunk 时退化为 SDK 帧号。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    /// 相机组里的序号（从 0 开始）
    pub cam: u8,
    pub frame_counter: u64,
    pub trigger_counter: u64,
    pub lost_packets: u32,
    pub ts: i64,
    /// 软触发（示教取图、回放"下一张"）出来的帧
    #[serde(skip)]
    pub manual: bool,
    /// 整帧 Mono8 像素。图像测量、帧录制或手动取图时才带上。
    #[serde(skip)]
    pub image: Option<Arc<FrameImage>>,
}
