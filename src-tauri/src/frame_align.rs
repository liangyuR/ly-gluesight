//! 分段标定的图像辅助（D-SB-44）：采集帧下采样、缩略图条带编码。只服务标定与复核，不进生产判定。

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

#[cfg(test)]
mod tests {
    use super::*;

    /// 前端按头部 [帧组数, 宽, 高] + 每组 1 字节状态 + 灰度逐组解码：缺帧占位且状态为 0，帧组顺序不变，
    /// 下采样按块取均值（四舍五入）。
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
