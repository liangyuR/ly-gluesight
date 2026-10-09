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

    /// 8 位 Bayer 转成灰度写进池里的缓冲（见 [`bayer8_gray_into`]）。
    pub fn bayer8_to_gray(&self, width: u32, height: u32, src: &[u8]) -> Result<FrameImage, String> {
        let mut buf = self.0.free.lock().unwrap().pop().unwrap_or_default();
        if let Err(e) = bayer8_gray_into(width as usize, height as usize, src, &mut buf) {
            self.0.put(buf);
            return Err(e);
        }
        Ok(FrameImage { width, height, pixels: buf, pool: Some(Arc::downgrade(&self.0)) })
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
    /// 模拟、回放相机自己编的号：每次触发加一，（重新）加载时从 0 重来
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
    /// 设备会话号：海康每打开一次、模拟 / 回放每（重新）加载一次换一个新号，进程内不重复。
    /// 会话变了，触发计数可能从头数起
    pub session: u64,
    /// 触发计数的来源
    pub counter: CounterSource,
    pub frame_counter: u64,
    pub trigger_counter: u64,
    pub lost_packets: u32,
    pub ts: i64,
    /// 软触发（示教取图、回放"下一张"）出来的帧
    #[serde(skip)]
    pub manual: bool,
    /// 整帧 8 位灰度（Mono8 原样，8 位 Bayer 已转灰度）。图像测量、帧录制或手动取图时才带上。
    #[serde(skip)]
    pub image: Option<Arc<FrameImage>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2×2 周期里各位置的通道（0 = R，1 = G，2 = B），按 [行][列]。
    const PHASES: [(&str, [[usize; 2]; 2]); 4] = [
        ("BG", [[2, 1], [1, 0]]),
        ("RG", [[0, 1], [1, 2]]),
        ("GB", [[1, 2], [0, 1]]),
        ("GR", [[1, 0], [2, 1]]),
    ];

    fn mosaic(phase: [[usize; 2]; 2], w: usize, h: usize, rgb: impl Fn(usize, usize) -> [u8; 3]) -> Vec<u8> {
        (0..h).flat_map(|y| (0..w).map(move |x| (x, y))).map(|(x, y)| rgb(x, y)[phase[y % 2][x % 2]]).collect()
    }

    fn gray(w: usize, h: usize, src: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        bayer8_gray_into(w, h, src, &mut out).unwrap();
        assert_eq!(out.len(), w * h);
        out
    }

    #[test]
    fn uniform_scene_gives_weighted_gray_in_every_phase() {
        let expect = ((200 + 2 * 100 + 40 + 2) / 4) as u8;
        assert_eq!(expect, 110);
        for (w, h) in [(8, 6), (5, 3), (2, 2), (7, 2), (2, 5)] {
            for (name, phase) in PHASES {
                let out = gray(w, h, &mosaic(phase, w, h, |_, _| [200, 100, 40]));
                assert!(out.iter().all(|&v| v == expect), "{name} {w}×{h}: {out:?}");
            }
        }
    }

    #[test]
    fn needs_at_least_2x2_and_enough_bytes() {
        let mut out = Vec::new();
        for (w, h) in [(1, 4), (4, 1), (1, 1), (0, 0), (0, 3)] {
            assert!(bayer8_gray_into(w, h, &[0; 16], &mut out).is_err(), "{w}×{h}");
        }
        assert!(bayer8_gray_into(4, 4, &[0; 15], &mut out).is_err());
        assert!(bayer8_gray_into(2, 2, &[0; 4], &mut out).is_ok());
    }

    #[test]
    fn phases_agree_on_the_same_scene() {
        // 色度不变、亮度逐像素变化：任一 2×2 块之和 = R0 + 2G0 + B0 + 块内 f 之和，与相位无关。
        // （色度突变处相位不同结果会不同，这是 2×2 求和本身的性质。）
        let (w, h) = (9, 7);
        let f = |x: usize, y: usize| ((x * 7 + y * 13) % 50) as u8;
        let scene = |x, y| [120 + f(x, y), 60 + f(x, y), 30 + f(x, y)];
        let outs: Vec<_> = PHASES.iter().map(|(_, p)| gray(w, h, &mosaic(*p, w, h, scene))).collect();
        for (i, (name, _)) in PHASES.iter().enumerate() {
            assert_eq!(outs[i], outs[0], "{name}");
        }
        for y in 0..h {
            for x in 0..w {
                let (bx, by) = (x.min(w - 2), y.min(h - 2));
                let s: u32 = [(bx, by), (bx + 1, by), (bx, by + 1), (bx + 1, by + 1)].iter().map(|&(x, y)| f(x, y) as u32).sum();
                assert_eq!(outs[0][y * w + x] as u32, (120 + 2 * 60 + 30 + s + 2) / 4, "({x}, {y})");
            }
        }
    }

    #[test]
    fn pool_converts_into_reused_buffer() {
        let pool = FramePool::new(2);
        let src = mosaic(PHASES[0].1, 6, 4, |_, _| [200, 100, 40]);
        let img = pool.bayer8_to_gray(6, 4, &src).unwrap();
        assert_eq!((img.width, img.height, img.pixels.len()), (6, 4, 24));
        assert!(img.pixels.iter().all(|&v| v == 110));
        drop(img);
        assert_eq!(pool.0.free.lock().unwrap().len(), 1);
        assert!(pool.bayer8_to_gray(1, 4, &src).is_err());
        // 出错时缓冲放回池里
        assert_eq!(pool.0.free.lock().unwrap().len(), 1);
    }

    #[test]
    fn time_full_frame_conversion() {
        let (w, h) = (1280, 1024);
        let src: Vec<u8> = (0..w * h).map(|i| (i * 31 % 251) as u8).collect();
        let mut out = Vec::new();
        bayer8_gray_into(w, h, &src, &mut out).unwrap();
        let started = std::time::Instant::now();
        let n = 10;
        for _ in 0..n {
            bayer8_gray_into(w, h, std::hint::black_box(&src), &mut out).unwrap();
            std::hint::black_box(&out);
        }
        let ms = started.elapsed().as_secs_f64() * 1000.0 / n as f64;
        // 本 crate 在 dev / test 配置下也是 opt-level 2（见 Cargo.toml）
        eprintln!("Bayer → 灰度 {w}×{h}：{ms:.2} ms / 帧");
    }
}
