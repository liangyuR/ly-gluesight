use std::collections::{HashMap, HashSet, VecDeque};
use crate::frame::FrameImage;
use crate::recipe::Polarity;

#[derive(Clone, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrushGuide {
    pub points: Vec<[f32; 2]>,
    pub radius_px: f32,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ExtractionFailure {
    pub point: [f32; 2],
    pub reason: &'static str,
    pub message: String,
}

#[derive(Debug)]
pub(crate) enum ExtractionError {
    Invalid(String),
    Failed(ExtractionFailure),
}

impl From<&str> for ExtractionError {
    fn from(message: &str) -> Self { Self::Invalid(message.into()) }
}

impl From<String> for ExtractionError {
    fn from(message: String) -> Self { Self::Invalid(message) }
}

fn failure(point: [f32; 2], reason: &'static str, message: &str) -> ExtractionError {
    ExtractionError::Failed(ExtractionFailure { point, reason, message: message.into() })
}

#[derive(Debug)]
pub(crate) struct Extraction {
    pub path: Vec<[f32; 2]>,
    pub notice: Option<String>,
}

/// 中线起点前、终点后截去的胶路超过这个长度时提示操作员核对。
const TRIM_NOTICE_PX: f32 = 10.0;

fn trim_notice(lost: [f32; 2]) -> Option<String> {
    let parts: Vec<String> = [("起点前", lost[0]), ("终点后", lost[1])].into_iter()
        .filter(|(_, px)| *px >= TRIM_NOTICE_PX).map(|(end, px)| format!("{end}约 {px:.0} px")).collect();
    (!parts.is_empty()).then(|| format!("中线已提取，但{} 未能连续找到胶边，已截去。请核对中线是否覆盖整条胶路，再运行试测。", parts.join("、")))
}

pub(crate) fn extract(image: &FrameImage, roi: [u32; 4], polarity: Polarity, guide: Option<&BrushGuide>, point_spacing_px: f32) -> Result<Extraction, ExtractionError> {
    if !point_spacing_px.is_finite() || !(4.0..=80.0).contains(&point_spacing_px) {
        return Err("中线点间距需为 4–80 像素".into());
    }
    let [x, y, width, height] = roi;
    if width < 5 || height < 5 || x.checked_add(width).is_none_or(|v| v > image.width) || y.checked_add(height).is_none_or(|v| v > image.height)
        || u64::from(width) * u64::from(height) > 4_000_000 || image.pixels.len() as u64 != u64::from(image.width) * u64::from(image.height) {
        return Err("请在原图内框选 5×5 以上、400 万像素以内的胶路区域".into());
    }
    if let Some(guide) = guide {
        let (path, trimmed) = extract_guided(image, roi, polarity, guide, point_spacing_px)?;
        return Ok(Extraction { path, notice: trim_notice(trimmed) });
    }
    extract_box(image, roi, polarity, point_spacing_px)
}

fn otsu(histogram: &[usize; 256]) -> Option<usize> {
    let total = histogram.iter().sum::<usize>() as f64;
    let sum: f64 = histogram.iter().enumerate().map(|(i, n)| i as f64 * *n as f64).sum();
    let (mut count, mut partial, mut best, mut threshold) = (0.0, 0.0, 0.0, 0usize);
    for (i, n) in histogram.iter().enumerate() {
        count += *n as f64; partial += i as f64 * *n as f64;
        if count == 0.0 || count == total { continue; }
        let variance = count * (total - count) * (partial / count - (sum - partial) / (total - count)).powi(2);
        if variance > best { best = variance; threshold = i; }
    }
    (best > 0.0).then_some(threshold)
}

/// 一维滑动窗口取最小 / 最大值（窗口在边界处截断），rows 为 true 时沿行做。
fn sweep(src: &[u8], w: usize, h: usize, size: usize, max: bool, rows: bool) -> Vec<u8> {
    let (lines, len, stride, step) = if rows { (h, w, w, 1) } else { (w, h, 1, w) };
    let r = size / 2;
    let mut out = vec![0u8; src.len()];
    let mut queue = VecDeque::new();
    for line in 0..lines {
        let at = |i: usize| line * stride + i * step;
        queue.clear();
        let mut next = 0;
        for i in 0..len {
            while next <= (i + r).min(len - 1) {
                let v = src[at(next)];
                while queue.back().is_some_and(|&b| if max { src[at(b)] <= v } else { src[at(b)] >= v }) { queue.pop_back(); }
                queue.push_back(next);
                next += 1;
            }
            while queue.front().is_some_and(|&f| f + r < i) { queue.pop_front(); }
            out[at(i)] = src[at(queue[0])];
        }
    }
    out
}

/// 胶色一侧的顶帽响应（方形结构元开运算后相减），再做 5 点二项式平滑：比结构元宽的成片暗区（亮胶为亮区）响应近 0。
fn top_hat(image: &FrameImage, polarity: Polarity, [x0, y0, x1, y1]: [usize; 4], size: usize) -> Vec<u8> {
    let (w, h) = (x1 - x0, y1 - y0);
    let stride = image.width as usize;
    let src: Vec<u8> = (y0..y1).flat_map(|r| (x0..x1).map(move |c| {
        let v = image.pixels[r * stride + c];
        if polarity == Polarity::Dark { 255 - v } else { v }
    })).collect();
    let eroded = sweep(&sweep(&src, w, h, size, false, true), w, h, size, false, false);
    let opened = sweep(&sweep(&eroded, w, h, size, true, true), w, h, size, true, false);
    let top: Vec<u16> = src.iter().zip(&opened).map(|(a, b)| u16::from(a.saturating_sub(*b))).collect();
    let blur = |src: &[u16], rows: bool| -> Vec<u16> {
        let (len, other) = if rows { (w, h) } else { (h, w) };
        let mut out = vec![0u16; src.len()];
        for line in 0..other {
            let at = |i: usize| if rows { line * w + i } else { i * w + line };
            for i in 0..len {
                let tap = |d: isize| src[at((i as isize + d).clamp(0, len as isize - 1) as usize)];
                out[at(i)] = (tap(-2) + 4 * tap(-1) + 6 * tap(0) + 4 * tap(1) + tap(2) + 8) / 16;
            }
        }
        out
    };
    blur(&blur(&top, true), false).into_iter().map(|v| v.min(255) as u8).collect()
}

/// 框选：顶帽（结构元取框的短边）突出比它窄的胶条、压掉框里成片的暗区，Otsu 二值化后取骨架最长的一条，
/// 去掉两端被框角拉弯的半个胶宽、再顺着端点切向伸出一个胶宽（不出框），当作笔划交给笔刷提取对中（伸出胶外的头尾由它截掉）；笔刷提取断开时从断处分开、留长的一段重试。
/// 骨架首尾相连（落在笔刷外的一段从两头接回骨架）时报错；笔刷提取截掉的、超出伸出段的胶路写进提示。
fn extract_box(image: &FrameImage, roi: [u32; 4], polarity: Polarity, spacing: f32) -> Result<Extraction, ExtractionError> {
    let [x, y, width, height] = roi;
    let size = (width.min(height).clamp(31, 161) | 1) as usize;
    let pad = [x as usize, y as usize].map(|v| v.saturating_sub(size));
    let far = [(x + width) as usize + size, (y + height) as usize + size];
    let region = [pad[0], pad[1], far[0].min(image.width as usize), far[1].min(image.height as usize)];
    let response = top_hat(image, polarity, region, size);
    let rw = region[2] - region[0];
    let at = |row: usize, col: usize| response[(y as usize - region[1] + row) * rw + x as usize - region[0] + col];
    let w = width as usize + 2;
    let h = height as usize + 2;
    let mut histogram = [0usize; 256];
    for row in 0..height as usize { for col in 0..width as usize { histogram[at(row, col) as usize] += 1; } }
    let threshold = otsu(&histogram).ok_or("区域内没有可分辨的胶路，请调整区域或手工绘制")?;
    let total = (width * height) as f64;
    let mut mask = vec![false; w * h];
    for row in 0..height as usize { for col in 0..width as usize { mask[(row + 1) * w + col + 1] = at(row, col) as usize > threshold; } }
    let offsets = [-(w as isize), -(w as isize) + 1, 1, w as isize + 1, w as isize, w as isize - 1, -1, -(w as isize) - 1];
    let neighbors = |i: usize| offsets.map(|d| (i as isize + d) as usize);
    let mut label = vec![usize::MAX; mask.len()];
    let mut areas = Vec::new();
    for seed in 0..mask.len() {
        if !mask[seed] || label[seed] != usize::MAX { continue; }
        let id = areas.len();
        let mut queue = VecDeque::from([seed]); label[seed] = id;
        let mut area = 0usize;
        while let Some(i) = queue.pop_front() {
            area += 1;
            for j in neighbors(i) { if mask[j] && label[j] == usize::MAX { label[j] = id; queue.push_back(j); } }
        }
        areas.push(area);
    }
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
    let mut starts: Vec<(usize, usize)> = Vec::new();
    let mut seen = vec![false; areas.len()];
    for (i, on) in mask.iter().enumerate() {
        if *on && !seen[label[i]] { seen[label[i]] = true; starts.push((i, label[i])); }
    }
    starts.sort_by_key(|(_, id)| std::cmp::Reverse(areas[*id]));
    let mut best: Option<(Vec<[f32; 2]>, f32, usize)> = None;
    for &(seed, id) in starts.iter().take(8).filter(|(_, id)| areas[*id] >= 12) {
        let (start, _) = walk(seed);
        let (mut end, parent) = walk(start);
        let mut path = vec![end];
        while end != start { end = parent[end]; path.push(end); }
        let points: Vec<_> = path.into_iter().map(|i| [(i % w - 1) as f32 + x as f32, (i / w - 1) as f32 + y as f32]).collect();
        let length: f32 = points.windows(2).map(|p| distance(p[0], p[1])).sum();
        if best.as_ref().is_none_or(|b| length > b.1) { best = Some((points, length, id)); }
    }
    let Some((skeleton, length, id)) = best.filter(|b| b.0.len() >= 8) else { return Err("提取的胶路太短，请手工绘制".into()) };
    if areas[id] as f64 > total * 0.9 { return Err("未找到清晰的单段胶路，请调整区域或极性".into()); }
    // 骨架比胶条短约一个胶宽：面积 ≈ 胶宽 × (骨架长 + 胶宽)
    let bead = (-length + (length * length + 4.0 * areas[id] as f32).sqrt()) * 0.5;
    let mut arc = 0.0;
    let mut kept = Vec::new();
    for (i, p) in skeleton.iter().enumerate() {
        if i > 0 { arc += distance(skeleton[i - 1], *p); }
        if arc >= 0.5 * bead && arc <= length - 0.5 * bead { kept.push(*p); }
    }
    if kept.len() < 2 || length - bead < 8.0 { return Err("提取的胶路太短，请手工绘制".into()); }
    let mut stroke = resample(&kept, 6.0);
    let inner = ((bead / 6.0).ceil() as usize).clamp(1, stroke.len() - 1);
    let reach = |from: [f32; 2], to: [f32; 2]| {
        let d = distance(from, to).max(1e-3);
        let p = [to[0] + (to[0] - from[0]) / d * bead, to[1] + (to[1] - from[1]) / d * bead];
        [p[0].clamp(x as f32, (x + width - 1) as f32), p[1].clamp(y as f32, (y + height - 1) as f32)]
    };
    let (head, tail) = (reach(stroke[inner], stroke[0]), reach(stroke[stroke.len() - 1 - inner], stroke[stroke.len() - 1]));
    let extension = [distance(head, stroke[0]), distance(tail, stroke[stroke.len() - 1])];
    stroke.insert(0, head);
    stroke.push(tail);
    stroke = resample(&stroke, 6.0);
    let radius = (1.5 * bead).clamp(3.0, 150.0);
    let skeleton: Vec<usize> = (0..mask.len()).filter(|&i| mask[i] && label[i] == id).collect();
    let place: HashMap<usize, (f32, f32)> = skeleton.iter().map(|&i| (i, project([(i % w - 1) as f32 + x as f32, (i / w - 1) as f32 + y as f32], &stroke))).collect();
    let mut visited = HashSet::new();
    let closed = skeleton.iter().any(|&seed| {
        if place[&seed].0 <= radius || !visited.insert(seed) { return false; }
        let (mut low, mut high) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut queue = VecDeque::from([seed]);
        while let Some(i) = queue.pop_front() {
            for j in neighbors(i) {
                let Some(&(gap, arc)) = place.get(&j) else { continue };
                if gap <= radius { low = low.min(arc); high = high.max(arc); } else if visited.insert(j) { queue.push_back(j); }
            }
        }
        high - low > 2.0 * radius
    });
    if closed { return Err("框内胶路首尾相连，一条中线只能覆盖其中一段。请只框选其中一段胶路，或改用笔刷".into()); }
    let span = |s: &[[f32; 2]]| s.windows(2).map(|p| distance(p[0], p[1])).sum::<f32>();
    let full = span(&stroke);
    let mut dropped = [0.0f32; 2];
    let mut retries = 0;
    loop {
        match extract_guided(image, roi, polarity, &BrushGuide { points: stroke.clone(), radius_px: radius }, spacing) {
            Ok((path, trimmed)) => {
                let lost = [0, 1].map(|end| (dropped[end] + trimmed[end] - extension[end]).max(0.0));
                return Ok(Extraction { path, notice: trim_notice(lost) });
            }
            Err(ExtractionError::Failed(f)) if retries < 2 && f.reason != "reversal" => {
                let j = (0..stroke.len()).min_by(|a, b| distance(stroke[*a], f.point).total_cmp(&distance(stroke[*b], f.point))).unwrap();
                let (head_end, tail_start) = (j.saturating_sub(1), (j + 2).min(stroke.len()));
                let keep = if span(&stroke[..head_end]) >= span(&stroke[tail_start..]) { 0..head_end } else { tail_start..stroke.len() };
                if keep.len() < 2 || span(&stroke[keep.clone()]) < (0.5 * full).max(8.0) { return Err(box_failure(f)); }
                dropped[0] += span(&stroke[..=keep.start]);
                dropped[1] += span(&stroke[keep.end - 1..]);
                stroke = stroke[keep].to_vec();
                retries += 1;
            }
            Err(ExtractionError::Failed(f)) => return Err(box_failure(f)),
            Err(e) => return Err(e),
        }
    }
}

/// 笔刷提取的失败文案指向笔划和"笔划生成中线"，框选时没有笔划，改成框选能照做的建议。
fn box_failure(f: ExtractionFailure) -> ExtractionError {
    let message = match f.reason {
        "missing_edges" => "标记处未找到完整的两侧胶路边缘。请调整框选范围避开对比度不足处，或改用笔刷沿胶路刷选。",
        "discontinuity" => "标记处胶路边缘无法连续连接，局部中心位置或宽度变化过大。请缩小框选范围避开弯折、亮边或阴影，或改用笔刷沿胶路刷选。",
        "ambiguous" => "标记处有多条同样可信的胶路候选。请缩小框选范围排除邻近阴影，或改用笔刷沿目标胶路刷选。",
        _ => "标记处胶路折返，无法确定方向。请只框选单向的一段胶路，或改用笔刷沿胶路刷选。",
    };
    ExtractionError::Failed(ExtractionFailure { message: message.into(), ..f })
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

/// 点到折线的距离，以及最近点沿折线的弧长。
fn project(p: [f32; 2], line: &[[f32; 2]]) -> (f32, f32) {
    let (mut best, mut arc) = ((f32::INFINITY, 0.0), 0.0);
    for s in line.windows(2) {
        let d = [s[1][0] - s[0][0], s[1][1] - s[0][1]];
        let length = d[0].hypot(d[1]);
        let t = if length > 0.0 { (((p[0] - s[0][0]) * d[0] + (p[1] - s[0][1]) * d[1]) / (length * length)).clamp(0.0, 1.0) } else { 0.0 };
        let gap = distance(p, [s[0][0] + d[0] * t, s[0][1] + d[1] * t]);
        if gap < best.0 { best = (gap, arc + t * length); }
        arc += length;
    }
    best
}

fn resample(points: &[[f32; 2]], spacing: f32) -> Vec<[f32; 2]> {
    let mut result = vec![points[0]];
    let mut next = spacing;
    let mut traversed = 0.0;
    for pair in points.windows(2) {
        let length = distance(pair[0], pair[1]);
        if length < 0.0001 { continue; }
        while next < traversed + length {
            let t = (next - traversed) / length;
            result.push([pair[0][0] + t * (pair[1][0] - pair[0][0]), pair[0][1] + t * (pair[1][1] - pair[0][1])]);
            next += spacing;
        }
        traversed += length;
    }
    let end = *points.last().unwrap();
    if distance(*result.last().unwrap(), end) > 0.01 { result.push(end); }
    result
}

fn resample_output(points: &[[f32; 2]], spacing: f32) -> Vec<[f32; 2]> {
    let mut anchors = vec![false; points.len()];
    anchors[0] = true;
    anchors[points.len()-1] = true;
    let mut pending = vec![(0, points.len()-1)];
    while let Some((first, last)) = pending.pop() {
        let a = points[first];
        let b = points[last];
        let delta = [b[0]-a[0], b[1]-a[1]];
        let squared = delta[0]*delta[0]+delta[1]*delta[1];
        let mut farthest = None;
        let mut maximum = 0.9;
        for (i, &point) in points.iter().enumerate().take(last).skip(first+1) {
            let ratio = if squared > 0.0 { (((point[0]-a[0])*delta[0]+(point[1]-a[1])*delta[1])/squared).clamp(0.0,1.0) } else { 0.0 };
            let error = distance(point,[a[0]+ratio*delta[0],a[1]+ratio*delta[1]]);
            if error > maximum { maximum=error; farthest=Some(i); }
        }
        if let Some(i) = farthest { anchors[i]=true; pending.push((first,i)); pending.push((i,last)); }
    }
    let indices: Vec<_> = anchors.iter().enumerate().filter_map(|(i, keep)| keep.then_some(i)).collect();
    let mut result = vec![points[0]];
    for pair in indices.windows(2) {
        let a=points[pair[0]];
        let b=points[pair[1]];
        let segments=(distance(a,b)/spacing).ceil().max(1.0) as usize;
        for i in 1..segments { let t=i as f32/segments as f32; result.push([a[0]+(b[0]-a[0])*t,a[1]+(b[1]-a[1])*t]); }
        result.push(b);
    }
    result
}

fn gray(image: &FrameImage, point: [f32; 2]) -> Option<f32> {
    let [x, y] = point;
    if x < 0.0 || y < 0.0 || x > (image.width - 1) as f32 || y > (image.height - 1) as f32 { return None; }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(image.width as usize - 1), (y0 + 1).min(image.height as usize - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let pixel = |x, y| image.pixels[y * image.width as usize + x] as f32;
    Some((pixel(x0,y0) * (1.0-tx) + pixel(x1,y0) * tx) * (1.0-ty) + (pixel(x0,y1) * (1.0-tx) + pixel(x1,y1) * tx) * ty)
}

/// 胶面反光会把一条胶劈成两段：缝不宽于较宽一段时并成一条候选，只在逐站连不上时拿来重试，
/// 否则胶旁隔着窄缝的阴影也会被并进来，与胶本身成为两条同样可信的候选。
fn highlight_union(p: (f32, f32), q: (f32, f32)) -> Option<(f32, f32)> {
    let gap = q.0 - p.1;
    (gap > 0.0 && gap <= (p.1 - p.0).max(q.1 - q.0)).then_some((p.0, q.1))
}

/// 法向取前后各 NORMAL_REACH 个采样点（3 px 一个）连线的垂线，笔划上的小折角不会把剖面拧斜。
const NORMAL_REACH: usize = 4;

/// 返回中线及笔划起点前、终点后截去的长度（px）。
fn extract_guided(image: &FrameImage, roi: [u32; 4], polarity: Polarity, guide: &BrushGuide, spacing: f32) -> Result<(Vec<[f32; 2]>, [f32; 2]), ExtractionError> {
    if !guide.radius_px.is_finite() || !(3.0..=150.0).contains(&guide.radius_px) || !(2..=4096).contains(&guide.points.len()) {
        return Err("请沿胶路绘制笔划，笔刷半径需为 3–150 像素".into());
    }
    let [x, y, w, h] = roi;
    if guide.points.iter().any(|p| !p[0].is_finite() || !p[1].is_finite() || p[0] < x as f32 || p[1] < y as f32 || p[0] >= (x+w) as f32 || p[1] >= (y+h) as f32) {
        return Err("笔划必须位于图像选区内".into());
    }
    let length: f32 = guide.points.windows(2).map(|p| distance(p[0],p[1])).sum();
    if !(8.0..=20000.0).contains(&length) { return Err("笔划长度需为 8–20000 像素".into()); }
    let samples = resample(&guide.points, 3.0);
    let radius = guide.radius_px.floor() as i32;
    // 剖面比笔刷多伸出半个半径：笔划偏向胶的一侧时，另一侧的胶边落在笔刷外也能量到；候选中心仍须在笔刷内
    let reach = radius + radius / 2;
    let mut all = Vec::new();
    let mut relaxed = Vec::new();
    let mut widest = Vec::new();
    for (i, &center) in samples.iter().enumerate() {
        let before = samples[i.saturating_sub(NORMAL_REACH)];
        let after = samples[(i+NORMAL_REACH).min(samples.len()-1)];
        let len = distance(before, after);
        if len < 0.5 { return Err(failure(center, "reversal", "标记处笔划折返，无法确定胶路方向。请沿单段胶路单向重画，或用笔划生成中线后手工修正。")); }
        let normal = [-(after[1]-before[1])/len, (after[0]-before[0])/len];
        let profile: Vec<_> = (-reach..=reach).map(|d| {
            gray(image, [center[0]+normal[0]*d as f32, center[1]+normal[1]*d as f32]).map(|value| if polarity == Polarity::Dark { 255.0-value } else { value })
        }).collect();
        let smooth: Vec<_> = (0..profile.len()).map(|j| {
            if j == 0 || j+1 == profile.len() { profile[j] } else {
                match (profile[j-1], profile[j], profile[j+1]) { (Some(a),Some(b),Some(c)) => Some((a+2.0*b+c)/4.0), _ => None }
            }
        }).collect();
        let mut spans: Vec<(f32, f32)> = Vec::new();
        let mut j = 1;
        while j+1 < smooth.len() {
            let Some(peak) = smooth[j] else { j+=1; continue; };
            let mut end = j;
            while end+1 < smooth.len() && smooth[end+1] == Some(peak) { end+=1; }
            if smooth[j-1].is_some_and(|v| v < peak) && end+1 < smooth.len() && smooth[end+1].is_some_and(|v| v < peak) {
                let mut left = j-1;
                let mut left_min = smooth[left].unwrap();
                while left > 0 && smooth[left-1].is_some_and(|v| v <= peak) { left-=1; left_min=left_min.min(smooth[left].unwrap()); }
                let mut right = end+1;
                let mut right_min = smooth[right].unwrap();
                while right+1 < smooth.len() && smooth[right+1].is_some_and(|v| v <= peak) { right+=1; right_min=right_min.min(smooth[right].unwrap()); }
                let base = left_min.max(right_min);
                if peak-base >= 8.0 {
                    let half = base+(peak-base)*0.5;
                    let mut l = j;
                    while l > left && smooth[l-1].unwrap() >= half { l-=1; }
                    let mut r = end;
                    while r < right && smooth[r+1].unwrap() >= half { r+=1; }
                    if l > 0 && r+1 < smooth.len() {
                        if let (Some(lv),Some(rv)) = (smooth[l-1],smooth[r+1]) {
                            if lv < half && rv < half {
                                let a = l as f32 - (smooth[l].unwrap()-half)/(smooth[l].unwrap()-lv);
                                let b = r as f32 + (smooth[r].unwrap()-half)/(smooth[r].unwrap()-rv);
                                if b-a >= 2.0 { spans.push((a, b)); }
                            }
                        }
                    }
                }
            }
            j=end+1;
        }
        let inside = |(a, b): &(f32, f32)| ((a+b)*0.5-reach as f32).abs() <= radius as f32;
        spans.retain(inside);
        spans.sort_by(|x, y| x.0.total_cmp(&y.0));
        let candidate = |(a, b): (f32, f32)| {
            let offset = (a+b)*0.5-reach as f32;
            (offset, b-a, [center[0]+normal[0]*offset,center[1]+normal[1]*offset])
        };
        let candidates: Vec<_> = spans.iter().copied().map(candidate).collect();
        let fresh = |list: &[(f32, f32)], u: &(f32, f32)| !spans.iter().chain(list).any(|s| (s.0-u.0).abs() < 0.5 && (s.1-u.1).abs() < 0.5);
        let mut adjacent: Vec<(f32, f32)> = Vec::new();
        for u in spans.windows(2).filter_map(|p| highlight_union(p[0], p[1])).filter(inside) {
            if fresh(&adjacent, &u) { adjacent.push(u); }
        }
        let mut wide = adjacent.clone();
        for u in spans.iter().flat_map(|&p| spans.iter().filter_map(move |&q| highlight_union(p, q))).filter(inside) {
            if fresh(&wide, &u) { wide.push(u); }
        }
        relaxed.push(candidates.iter().copied().chain(adjacent.into_iter().map(candidate)).collect::<Vec<_>>());
        widest.push(candidates.iter().copied().chain(wide.into_iter().map(candidate)).collect::<Vec<_>>());
        all.push(candidates);
    }
    // 笔划两头伸出胶路（喷嘴、图像边缘）时，两头各至多四分之一的采样点找不到胶边就截掉，截去的长度交给调用方提示
    let mut longest = 0..0;
    let mut start = 0;
    for i in 0..=all.len() {
        if i < all.len() && !all[i].is_empty() { continue; }
        if i - start > longest.len() { longest = start..i; }
        start = i + 1;
    }
    let trim = samples.len() / 4;
    if longest.len() < 3 || longest.start > trim || samples.len() - longest.end > trim {
        // 标在挡住最长一段的那个缺口的起点，而不是头尾可截范围里的第一个空站
        let gap = if longest.start > trim { (0..longest.start).rev().take_while(|&i| all[i].is_empty()).last() } else { (longest.end..all.len()).find(|&i| all[i].is_empty()) };
        let point = samples[gap.or_else(|| all.iter().position(Vec::is_empty)).unwrap_or(0)];
        return Err(failure(point, "missing_edges", "标记处未找到完整的两侧胶路边缘。请检查笔刷是否覆盖两侧边缘及少量背景、图像对比度是否足够，或用笔划生成中线后手工修正。"));
    }
    // 并集只在逐站连不上时重试：先只并相邻两段；仍连不上，再并隔着一段（高光里的小暗点）的两段
    let solve = |keep: std::ops::Range<usize>| {
        let (samples, all) = (&samples[keep.clone()], &all[keep.clone()]);
        let result = trace(all, samples, guide.radius_px, spacing);
        for retry in [&relaxed[keep.clone()], &widest[keep.clone()]] {
            let Err(ExtractionError::Failed(f)) = &result else { break };
            if f.reason != "discontinuity" || retry.iter().zip(all).all(|(r, s)| r.len() == s.len()) { break; }
            match trace(retry, samples, guide.radius_px, spacing) {
                Ok(path) => return Ok(path),
                Err(ExtractionError::Failed(g)) if g.reason == "discontinuity" => continue,
                Err(_) => break,
            }
        }
        result
    };
    // 连不上的地方落在笔划头尾四分之一以内（笔划拐出胶路）：截掉那一头再连一次
    let (path, kept) = match solve(longest.clone()) {
        Err(ExtractionError::Failed(f)) if f.reason == "discontinuity" => {
            let at = (longest.clone()).find(|&i| samples[i] == f.point).unwrap_or(longest.start);
            let rest = if at + trim >= samples.len() { longest.start..at } else if at <= trim { at..longest.end } else { 0..0 };
            if rest.len() < 3 { return Err(ExtractionError::Failed(f)); }
            (solve(rest.clone()).map_err(|_| ExtractionError::Failed(f))?, rest)
        }
        result => (result?, longest),
    };
    let arc = |s: &[[f32; 2]]| s.windows(2).map(|p| distance(p[0], p[1])).sum::<f32>();
    Ok((path, [arc(&samples[..=kept.start]), arc(&samples[kept.end - 1..])]))
}

type Candidate = (f32, f32, [f32; 2]);

fn trace(all: &[Vec<Candidate>], samples: &[[f32; 2]], radius: f32, spacing: f32) -> Result<Vec<[f32; 2]>, ExtractionError> {
    let mut costs: Vec<f32> = all[0].iter().map(|c| 8.0*(c.0/radius).powi(2)).collect();
    let mut parents = vec![vec![0; all[0].len()]];
    for i in 1..all.len() {
        let mut next = vec![f32::INFINITY; all[i].len()];
        let mut parent = vec![0; all[i].len()];
        for (j, c) in all[i].iter().enumerate() {
            for (k, previous) in all[i-1].iter().enumerate() {
                let move_px = distance(c.2,previous.2);
                if move_px > 9.0 || (c.1-previous.1).abs() > previous.1.min(c.1).max(8.0)*0.6 { continue; }
                let cost = costs[k] + 8.0*(c.0/radius).powi(2) + ((c.0-previous.0)/3.0).powi(2) + ((c.1-previous.1)/previous.1.max(3.0)).powi(2);
                if cost < next[j] { next[j]=cost; parent[j]=k; }
            }
        }
        if next.iter().all(|v| !v.is_finite()) { return Err(failure(samples[i], "discontinuity", "标记处候选边缘无法连续连接，局部中心位置或宽度变化过大。请检查弯折、亮边或阴影，并调整局部笔划，或用笔划生成中线后手工修正。")); }
        costs=next; parents.push(parent);
    }
    let mut selected = costs.iter().enumerate().min_by(|a,b| a.1.total_cmp(b.1)).unwrap().0;
    if let Some((mut alternate, _)) = costs.iter().enumerate().find(|(i, cost)| *i != selected && (all.last().unwrap()[*i].0-all.last().unwrap()[selected].0).abs() > 3.0 && (**cost-costs[selected])/(samples.len() as f32) < 0.08) {
        let mut primary = selected;
        let mut largest = 0.0;
        let mut point = *samples.last().unwrap();
        for i in (0..all.len()).rev() {
            let separation = distance(all[i][primary].2, all[i][alternate].2);
            if separation > largest { largest = separation; point = samples[i]; }
            primary = parents[i][primary];
            alternate = parents[i][alternate];
        }
        return Err(failure(point, "ambiguous", "标记处有多条同样可信的胶路候选。请把笔划移近目标胶路并排除邻近阴影，或用笔划生成中线后手工修正。"));
    }
    let mut result = Vec::with_capacity(all.len());
    for i in (0..all.len()).rev() { result.push(all[i][selected].2); selected=parents[i][selected]; }
    result.reverse();
    Ok(resample_output(&result, spacing))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 暗胶 x = 80–93 旁边是更宽的暗阴影 x = 24–59；画笔沿胶画，半径大到能扫到阴影。
    fn glue_beside_wider_shadow() -> (FrameImage, BrushGuide) {
        let mut image = FrameImage::new(160, 140, vec![230; 22400]);
        for y in 0..140 { for x in 24..60 { image.pixels[y*160+x]=55; } for x in 80..94 { image.pixels[y*160+x]=25 + ((x*17+y*11)%7) as u8; } }
        (image, BrushGuide { points: vec![[85.0,10.0],[88.0,70.0],[85.0,130.0]], radius_px: 65.0 })
    }

    #[test]
    fn brush_follows_requested_glue_instead_of_larger_shadow() {
        let (mut image, guide) = glue_beside_wider_shadow();
        for polarity in [Polarity::Dark, Polarity::Light] {
            let path = extract(&image,[0,0,160,140],polarity,Some(&guide),4.0).unwrap().path;
            assert!(path.iter().all(|p| (85.0..=88.0).contains(&p[0])), "{polarity:?}");
            assert!(path.windows(2).all(|p| p[1][1] > p[0][1]), "{polarity:?}");
            for pixel in &mut image.pixels { *pixel=255-*pixel; }
        }
    }

}
