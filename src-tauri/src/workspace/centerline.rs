use std::collections::VecDeque;
use crate::frame::FrameImage;
use crate::recipe::Polarity;

pub(super) fn extract(image: &FrameImage, roi: [u32; 4], polarity: Polarity) -> Result<Vec<[f32; 2]>, String> {
    let [x, y, width, height] = roi;
    if width < 5 || height < 5 || x.checked_add(width).is_none_or(|v| v > image.width) || y.checked_add(height).is_none_or(|v| v > image.height)
        || u64::from(width) * u64::from(height) > 4_000_000 || image.pixels.len() as u64 != u64::from(image.width) * u64::from(image.height) {
        return Err("请在原图内框选 5×5 以上、400 万像素以内的胶路区域".into());
    }
    let w = width as usize + 2;
    let h = height as usize + 2;
    let mut histogram = [0usize; 256];
    for row in y..y + height { for col in x..x + width { histogram[image.pixels[(row * image.width + col) as usize] as usize] += 1; } }
    let total = (width * height) as f64;
    let sum: f64 = histogram.iter().enumerate().map(|(i, n)| i as f64 * *n as f64).sum();
    let (mut count, mut partial, mut best, mut threshold) = (0.0, 0.0, 0.0, 0usize);
    for (i, n) in histogram.iter().enumerate() {
        count += *n as f64; partial += i as f64 * *n as f64;
        if count == 0.0 || count == total { continue; }
        let variance = count * (total - count) * (partial / count - (sum - partial) / (total - count)).powi(2);
        if variance > best { best = variance; threshold = i; }
    }
    if best == 0.0 { return Err("区域内没有可分辨的胶路，请调整区域或手工绘制".into()); }
    let mut mask = vec![false; w * h];
    for row in 0..height as usize { for col in 0..width as usize {
        let p = image.pixels[(y as usize + row) * image.width as usize + x as usize + col] as usize;
        mask[(row + 1) * w + col + 1] = if polarity == Polarity::Dark { p <= threshold } else { p > threshold };
    } }
    let offsets = [-(w as isize), -(w as isize) + 1, 1, w as isize + 1, w as isize, w as isize - 1, -1, -(w as isize) - 1];
    let neighbors = |i: usize| offsets.map(|d| (i as isize + d) as usize);
    let mut visited = vec![false; mask.len()];
    let mut largest = Vec::new();
    for seed in 0..mask.len() {
        if !mask[seed] || visited[seed] { continue; }
        let mut queue = VecDeque::from([seed]); visited[seed] = true;
        let mut component = Vec::new();
        while let Some(i) = queue.pop_front() {
            component.push(i);
            for j in neighbors(i) { if mask[j] && !visited[j] { visited[j] = true; queue.push_back(j); } }
        }
        if component.len() > largest.len() { largest = component; }
    }
    if largest.len() < 12 || largest.len() as f64 > total * 0.9 { return Err("未找到清晰的单段胶路，请调整区域或极性".into()); }
    mask.fill(false);
    for i in largest { mask[i] = true; }
    let mut converged = false;
    for _ in 0..256 {
        let mut changed = false;
        for second in [false, true] {
            let mut remove = Vec::new();
            for row in 1..h - 1 { for col in 1..w - 1 {
                let i = row * w + col;
                if !mask[i] { continue; }
                let p = neighbors(i).map(|j| mask[j]);
                let n = p.iter().filter(|v| **v).count();
                let changes = (0..8).filter(|j| !p[*j] && p[(*j + 1) % 8]).count();
                let keep = if second { (p[0] && p[2] && p[6]) || (p[0] && p[4] && p[6]) } else { (p[0] && p[2] && p[4]) || (p[2] && p[4] && p[6]) };
                if (2..=6).contains(&n) && changes == 1 && !keep { remove.push(i); }
            } }
            changed |= !remove.is_empty();
            for i in remove { mask[i] = false; }
        }
        if !changed { converged = true; break; }
    }
    if !converged { return Err("胶路区域过宽，无法可靠提取中线，请缩小范围或手工绘制".into()); }
    let tips: Vec<_> = mask.iter().enumerate().filter(|(i, on)| **on && neighbors(*i).iter().filter(|j| mask[**j]).count() == 1).map(|(i, _)| i).collect();
    if tips.len() < 2 { return Err("未识别出连续胶路的两个端点，请手工绘制".into()); }
    let walk = |start: usize| {
        let mut distance = vec![usize::MAX; mask.len()];
        let mut parent = vec![usize::MAX; mask.len()];
        let mut queue = VecDeque::from([start]); distance[start] = 0;
        let mut last = start;
        while let Some(i) = queue.pop_front() {
            last = i;
            for j in neighbors(i) { if mask[j] && distance[j] == usize::MAX { distance[j] = distance[i] + 1; parent[j] = i; queue.push_back(j); } }
        }
        (last, parent)
    };
    let (start, _) = walk(tips[0]);
    let (mut end, parent) = walk(start);
    let mut path = vec![end];
    while end != start { end = parent[end]; path.push(end); }
    if path.len() < 8 { return Err("提取的胶路太短，请手工绘制".into()); }
    let stride = path.len().div_ceil(256).max(2);
    let mut selected: Vec<_> = path.iter().step_by(stride).copied().collect();
    if selected.last() != path.last() { selected.push(*path.last().unwrap()); }
    Ok(selected.into_iter().map(|i| [(i % w - 1) as f32 + x as f32, (i / w - 1) as f32 + y as f32]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn extracts_continuous_line_and_rejects_uniform_or_outside_roi() {
        let mut image = FrameImage::new(100, 60, vec![230; 6000]);
        for y in 23..34 { for x in 10..90 { image.pixels[y * 100 + x] = 25; } }
        let path = extract(&image, [2, 2, 96, 56], Polarity::Dark).unwrap();
        assert!(path.len() > 10);
        assert!(path.iter().all(|p| (26.0..=30.0).contains(&p[1])));
        assert!(extract(&image, [95, 2, 20, 20], Polarity::Dark).is_err());
        image.pixels.fill(100);
        assert!(extract(&image, [0, 0, 100, 60], Polarity::Dark).is_err());
    }
}
