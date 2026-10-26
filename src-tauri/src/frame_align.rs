//! 分段标定的图像辅助（D-SB-44）：采集帧下采样、缩略图条带编码、按图像把模板轮的边界对齐到其他轮次。
//! 只服务标定与复核，结果只作草稿，不进生产判定。

use crate::frame::FrameImage;

/// 箱式平均下采样：每 factor × factor 个像素取均值，尾部不足一格的行列舍去；factor 为 0 或 1 时原样复制。
pub fn downsample(image: &FrameImage, factor: u32) -> FrameImage {
    let factor = factor.max(1);
    let (width, height) = (image.width / factor, image.height / factor);
    let area = factor * factor;
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let mut sum = 0u32;
            for dy in 0..factor {
                let row = ((y * factor + dy) * image.width + x * factor) as usize;
                sum += image.pixels[row..row + factor as usize]
                    .iter()
                    .map(|&p| u32::from(p))
                    .sum::<u32>();
            }
            pixels.push(((sum + area / 2) / area) as u8);
        }
    }
    FrameImage::new(width, height, pixels)
}

/// 一台相机一轮的缩略图条带：头部 [帧组数, 宽, 高]（u32 小端），之后按帧组 1..=N 每组 1 字节状态（1 有图、0 缺帧）
/// 加 宽 × 高 字节灰度（缺帧全 0）。尺寸取第一张有图的帧；尺寸不同的帧按缺帧处理，全部缺帧时宽高为 0。
pub fn thumb_strip(frames: &[Option<FrameImage>]) -> Vec<u8> {
    let (width, height) = frames
        .iter()
        .flatten()
        .next()
        .map_or((0, 0), |f| (f.width, f.height));
    let size = (width * height) as usize;
    let mut bytes = Vec::with_capacity(12 + frames.len() * (1 + size));
    for n in [frames.len() as u32, width, height] {
        bytes.extend(n.to_le_bytes());
    }
    for frame in frames {
        match frame
            .as_ref()
            .filter(|f| (f.width, f.height) == (width, height))
        {
            Some(frame) => {
                bytes.push(1);
                bytes.extend_from_slice(&frame.pixels);
            }
            None => {
                bytes.push(0);
                bytes.resize(bytes.len() + size, 0);
            }
        }
    }
    bytes
}

/// 一台相机一轮的帧组序列（按帧组 1..=N），缺帧为 None。
pub type Sequence = Vec<Option<FrameImage>>;

/// 对齐用小图的下采样倍数：1280×1024 → 40×32。
pub const ALIGN_FACTOR: u32 = 32;

fn mean_abs_diff(a: &FrameImage, b: &FrameImage) -> Option<f64> {
    if (a.width, a.height) != (b.width, b.height) || a.pixels.is_empty() {
        return None;
    }
    let sum: u64 = a
        .pixels
        .iter()
        .zip(&b.pixels)
        .map(|(&x, &y)| u64::from(x.abs_diff(y)))
        .sum();
    Some(sum as f64 / a.pixels.len() as f64)
}

/// 模板 N 帧 × 目标 M 帧的距离：每台相机两帧的平均绝对差除以该相机全部有效距离的中位数（抵消各相机曝光与纹理差异），
/// 再对两帧都在的相机取平均；三台都缺时取 1（中性）。
fn distances(template: &[Sequence], target: &[Sequence]) -> Vec<Vec<f64>> {
    let (n, m) = (
        template.first().map_or(0, Vec::len),
        target.first().map_or(0, Vec::len),
    );
    let mut sum = vec![vec![0.0; m]; n];
    let mut count = vec![vec![0u32; m]; n];
    for (a, b) in template.iter().zip(target) {
        let raw: Vec<Vec<Option<f64>>> = (0..n)
            .map(|i| {
                (0..m)
                    .map(|j| {
                        a.get(i)?
                            .as_ref()
                            .zip(b.get(j)?.as_ref())
                            .and_then(|(x, y)| mean_abs_diff(x, y))
                    })
                    .collect()
            })
            .collect();
        let mut all: Vec<f64> = raw.iter().flatten().flatten().copied().collect();
        if all.is_empty() {
            continue;
        }
        all.sort_by(f64::total_cmp);
        let median = all[all.len() / 2].max(1e-6);
        for i in 0..n {
            for j in 0..m {
                if let Some(d) = raw[i][j] {
                    sum[i][j] += d / median;
                    count[i][j] += 1;
                }
            }
        }
    }
    (0..n)
        .map(|i| {
            (0..m)
                .map(|j| {
                    if count[i][j] > 0 {
                        sum[i][j] / f64::from(count[i][j])
                    } else {
                        1.0
                    }
                })
                .collect()
        })
        .collect()
}

/// 首尾固定的 DTW 路径（0 起的 (模板帧, 目标帧)）：只在对角线（按长度比例缩放）两侧 band 帧内搜索。
fn dtw_path(d: &[Vec<f64>], band: usize) -> Vec<(usize, usize)> {
    let n = d.len();
    let m = d.first().map_or(0, Vec::len);
    if n == 0 || m == 0 {
        return Vec::new();
    }
    let inside = |i: usize, j: usize| {
        let diagonal = if n > 1 {
            i as f64 * (m - 1) as f64 / (n - 1) as f64
        } else {
            0.0
        };
        (j as f64 - diagonal).abs() <= band as f64
    };
    let mut acc = vec![vec![f64::INFINITY; m]; n];
    for i in 0..n {
        for j in 0..m {
            if !inside(i, j) {
                continue;
            }
            let previous = match (i, j) {
                (0, 0) => 0.0,
                (0, _) => acc[0][j - 1],
                (_, 0) => acc[i - 1][0],
                _ => acc[i - 1][j - 1].min(acc[i - 1][j]).min(acc[i][j - 1]),
            };
            acc[i][j] = d[i][j] + previous;
        }
    }
    let (mut i, mut j) = (n - 1, m - 1);
    let mut path = vec![(i, j)];
    while (i, j) != (0, 0) {
        (i, j) = match (i, j) {
            (0, _) => (0, j - 1),
            (_, 0) => (i - 1, 0),
            _ => {
                let steps = [(i - 1, j - 1), (i - 1, j), (i, j - 1)];
                *steps
                    .iter()
                    .min_by(|a, b| acc[a.0][a.1].total_cmp(&acc[b.0][b.1]))
                    .unwrap()
            }
        };
        path.push((i, j));
    }
    path.reverse();
    path
}

/// 一个边界的对齐建议。
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignedCut {
    /// 按图像对齐得到的边界（上一段最后一个帧组，从 1 起）
    pub cut: u64,
    /// 按总帧组数比例换算的边界（对照）
    pub proportional: u64,
    /// 需人工重点复核：与比例换算相差超过 2 帧，或为保持严格递增被挪动过
    pub review: bool,
}

/// 把模板轮的边界按图像对齐到目标轮：模板第 c 帧组之后的边界取 DTW 路径上模板帧 c、c + 1 所配目标帧（中位数）的中点，
/// 再保持严格递增、每段至少一帧。各相机的序列按计划相机顺序给出，模板与目标相机顺序一致。
pub fn align_cuts(template: &[Sequence], target: &[Sequence], cuts: &[u64]) -> Vec<AlignedCut> {
    let (n, m) = (
        template.first().map_or(0, Vec::len),
        target.first().map_or(0, Vec::len),
    );
    if n < 2 || m < 2 {
        return Vec::new();
    }
    let band = 3.max((m as f64 * 0.15).ceil() as usize);
    let path = dtw_path(&distances(template, target), band);
    let matched = |i: usize| {
        let mut js: Vec<usize> = path
            .iter()
            .filter(|&&(a, _)| a == i)
            .map(|&(_, b)| b)
            .collect();
        js.sort_unstable();
        js[js.len() / 2]
    };
    let mut previous = 0u64;
    cuts.iter()
        .enumerate()
        .map(|(k, &c)| {
            let c = c.clamp(1, n as u64 - 1);
            let proportional = (c * m as u64 + n as u64 / 2) / n as u64;
            let raw = ((matched(c as usize - 1) + matched(c as usize)) / 2) as u64 + 1;
            let remaining = (cuts.len() - 1 - k) as u64;
            let cut = raw.max(previous + 1).min(m as u64 - 1 - remaining);
            previous = cut;
            AlignedCut {
                cut,
                proportional,
                review: cut != raw || cut.abs_diff(proportional) > 2,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 前端按头部 [帧组数, 宽, 高] + 每组 1 字节状态 + 灰度逐组解码：缺帧占位且状态为 0，帧组顺序不变，
    /// 下采样按块取均值（四舍五入）。
    /// 合成序列：胶路位置 s 处的画面是两个沿相反方向移动的亮斑（每个 s 画面唯一）。
    fn scene(s: f64) -> Option<FrameImage> {
        let blob =
            |x: f64, y: f64, cx: f64, cy: f64| (-((x - cx).powi(2) + (y - cy).powi(2)) / 6.0).exp();
        let pixels = (0..32)
            .flat_map(|y| (0..40).map(move |x| (x as f64, y as f64)))
            .map(|(x, y)| {
                (20.0
                    + 200.0
                        * (blob(x, y, 4.0 + s, 10.0) + blob(x, y, 36.0 - 0.8 * s, 22.0)).min(1.0))
                    as u8
            })
            .collect();
        Some(FrameImage::new(40, 32, pixels))
    }

    /// D-SB-44 对齐复核：模板轮 30 帧、边界在胶路位置 7.5 / 14.5 / 22.5 之后；目标轮帧数不同（时间伸缩）、前慢后快
    /// （按比例换算差约 4 帧）、中途丢一帧或重复一帧时，按图像对齐给出的边界与真值（目标轮里位置小于边界位置的帧数）相差不超过 1 帧。
    #[test]
    fn aligned_cuts_follow_scene_positions_under_stretch_drop_and_repeat() {
        let template: Vec<Sequence> = (0..3)
            .map(|_| (0..30).map(|i| scene(i as f64)).collect())
            .collect();
        let cuts = [8, 15, 23];
        let stretched = |m: usize| {
            (0..m)
                .map(|j| j as f64 * 29.0 / (m - 1) as f64)
                .collect::<Vec<_>>()
        };
        let mut dropped = stretched(33);
        dropped.remove(12);
        let mut repeated = stretched(28);
        repeated.insert(18, repeated[18]);
        let uneven: Vec<f64> = (0..30)
            .map(|j| 29.0 * (j as f64 / 29.0).powf(1.6))
            .collect();
        for (case, positions) in [
            ("拉长到 36 帧", stretched(36)),
            ("压缩到 26 帧", stretched(26)),
            ("前慢后快", uneven),
            ("33 帧丢 1 帧", dropped),
            ("28 帧重复 1 帧", repeated),
        ] {
            let target: Vec<Sequence> = (0..3)
                .map(|_| positions.iter().map(|&s| scene(s)).collect())
                .collect();
            let aligned = align_cuts(&template, &target, &cuts);
            for (found, &cut) in aligned.iter().zip(&cuts) {
                let truth = positions.iter().filter(|&&s| s < cut as f64 - 0.5).count() as u64;
                assert!(
                    found.cut.abs_diff(truth) <= 1,
                    "{case}：模板边界 {cut} 真值 {truth}，对齐 {}",
                    found.cut
                );
            }
        }
    }

    #[test]
    fn thumb_strip_keeps_group_order_and_marks_missing_frames() {
        let image = |value: u8| {
            FrameImage::new(
                4,
                2,
                vec![
                    value,
                    value,
                    value + 2,
                    value + 2,
                    value,
                    value,
                    value + 2,
                    value + 2,
                ],
            )
        };
        let frames = vec![
            Some(downsample(&image(10), 2)),
            None,
            Some(downsample(&image(20), 2)),
        ];
        let bytes = thumb_strip(&frames);
        let header: Vec<u32> = bytes[..12]
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        assert_eq!(header, vec![3, 2, 1]);
        assert_eq!(&bytes[12..], &[1, 10, 12, 0, 0, 0, 1, 20, 22]);
    }
}
