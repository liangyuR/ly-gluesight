use std::collections::VecDeque;
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

pub(crate) fn extract(image: &FrameImage, roi: [u32; 4], polarity: Polarity, guide: Option<&BrushGuide>, point_spacing_px: f32) -> Result<Vec<[f32; 2]>, ExtractionError> {
    if !point_spacing_px.is_finite() || !(4.0..=80.0).contains(&point_spacing_px) {
        return Err("中线点间距需为 4–80 像素".into());
    }
    let [x, y, width, height] = roi;
    if width < 5 || height < 5 || x.checked_add(width).is_none_or(|v| v > image.width) || y.checked_add(height).is_none_or(|v| v > image.height)
        || u64::from(width) * u64::from(height) > 4_000_000 || image.pixels.len() as u64 != u64::from(image.width) * u64::from(image.height) {
        return Err("请在原图内框选 5×5 以上、400 万像素以内的胶路区域".into());
    }
    if let Some(guide) = guide {
        return extract_guided(image, roi, polarity, guide, point_spacing_px);
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
    let points: Vec<_> = path.into_iter().map(|i| [(i % w - 1) as f32 + x as f32, (i / w - 1) as f32 + y as f32]).collect();
    Ok(resample_output(&points, point_spacing_px))
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
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

fn extract_guided(image: &FrameImage, roi: [u32; 4], polarity: Polarity, guide: &BrushGuide, spacing: f32) -> Result<Vec<[f32; 2]>, ExtractionError> {
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
    let mut all = Vec::new();
    let mut relaxed = Vec::new();
    for (i, &center) in samples.iter().enumerate() {
        let before = samples[i.saturating_sub(2)];
        let after = samples[(i+2).min(samples.len()-1)];
        let len = distance(before, after);
        if len < 0.5 { return Err(failure(center, "reversal", "标记处笔划折返，无法确定胶路方向。请沿单段胶路单向重画，或用笔划生成中线后手工修正。")); }
        let normal = [-(after[1]-before[1])/len, (after[0]-before[0])/len];
        let profile: Vec<_> = (-radius..=radius).map(|d| {
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
        if spans.is_empty() { return Err(failure(center, "missing_edges", "标记处未找到完整的两侧胶路边缘。请检查笔刷是否覆盖两侧边缘及少量背景、图像对比度是否足够，或用笔划生成中线后手工修正。")); }
        spans.sort_by(|x, y| x.0.total_cmp(&y.0));
        let candidate = |(a, b): (f32, f32)| {
            let offset = (a+b)*0.5-radius as f32;
            (offset, b-a, [center[0]+normal[0]*offset,center[1]+normal[1]*offset])
        };
        let candidates: Vec<_> = spans.iter().copied().map(candidate).collect();
        let unions = spans.windows(2).filter_map(|p| highlight_union(p[0], p[1]))
            .filter(|u| !spans.iter().any(|s| (s.0-u.0).abs() < 0.5 && (s.1-u.1).abs() < 0.5))
            .map(candidate);
        relaxed.push(candidates.iter().copied().chain(unions).collect::<Vec<_>>());
        all.push(candidates);
    }
    match trace(&all, &samples, guide.radius_px, spacing) {
        Err(ExtractionError::Failed(f)) if f.reason == "discontinuity" && relaxed.iter().zip(&all).any(|(r, s)| r.len() > s.len()) => {
            trace(&relaxed, &samples, guide.radius_px, spacing).map_err(|_| ExtractionError::Failed(f))
        }
        result => result,
    }
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
                if move_px > 9.0 || (c.1-previous.1).abs() > previous.1.max(8.0)*0.6 { continue; }
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

    #[test]
    fn brush_follows_requested_glue_instead_of_larger_shadow_and_keeps_spacing_endpoints() {
        let mut image = FrameImage::new(160, 140, vec![230; 22400]);
        for y in 0..140 { for x in 24..60 { image.pixels[y*160+x]=55; } for x in 80..94 { image.pixels[y*160+x]=25 + ((x*17+y*11)%7) as u8; } }
        let guide = BrushGuide { points: vec![[85.0,10.0],[88.0,70.0],[85.0,130.0]], radius_px: 65.0 };
        for polarity in [Polarity::Dark, Polarity::Light] {
            let dense = extract(&image,[0,0,160,140],polarity,Some(&guide),4.0).unwrap();
            let sparse = extract(&image,[0,0,160,140],polarity,Some(&guide),30.0).unwrap();
            assert!(dense.iter().all(|p| (85.0..=88.0).contains(&p[0])));
            assert!(dense.windows(2).all(|p| p[1][1] > p[0][1]));
            assert!(sparse.len() < dense.len()/3);
            assert_eq!(dense.first(),sparse.first());
            assert_eq!(dense.last(),sparse.last());
            for pixel in &mut image.pixels { *pixel=255-*pixel; }
        }
        let mut corner = FrameImage::new(120,120,vec![230;14400]);
        for y in 10..105 { for x in 20..31 { corner.pixels[y*120+x]=25; } }
        for y in 94..105 { for x in 20..105 { corner.pixels[y*120+x]=25; } }
        let path = extract(&corner,[0,0,120,120],Polarity::Dark,None,80.0).unwrap();
        assert!(path.windows(2).all(|p| distance(p[0],p[1]) <= 80.01));
        for pair in path.windows(2) {
            for step in 0..=20 {
                let t=step as f32/20.0;
                let point=[pair[0][0]+(pair[1][0]-pair[0][0])*t,pair[0][1]+(pair[1][1]-pair[0][1])*t];
                assert!(gray(&corner,point).unwrap() < 100.0, "稀疏中线不能切角离开胶路");
            }
        }
    }

    #[test]
    fn bead_split_by_center_highlight_in_some_rows_still_yields_bead_centerline() {
        let mut image = FrameImage::new(160, 200, vec![230; 32000]);
        for y in 0..200 {
            for x in 60..94 { image.pixels[y*160+x] = 25; }
            if !(60..120).contains(&y) { for x in 74..80 { image.pixels[y*160+x] = 150; } }
        }
        let guide = BrushGuide { points: vec![[77.0,10.0],[77.0,190.0]], radius_px: 60.0 };
        let path = extract(&image,[0,0,160,200],Polarity::Dark,Some(&guide),20.0).unwrap();
        assert!(path.iter().all(|p| (p[0]-76.5).abs() <= 1.0), "{path:?}");
        assert!(path.first().unwrap()[1] <= 12.0 && path.last().unwrap()[1] >= 188.0);
    }

    #[test]
    fn glue_beside_narrow_shadow_across_grey_seam_yields_glue_center_not_ambiguous() {
        let mut image = FrameImage::new(160, 200, vec![230; 32000]);
        for y in 0..200 {
            for x in 60..74 { image.pixels[y*160+x] = 25; }
            for x in 74..77 { image.pixels[y*160+x] = 150; }
            for x in 77..83 { image.pixels[y*160+x] = 100; }
        }
        let guide = BrushGuide { points: vec![[67.0,10.0],[67.0,190.0]], radius_px: 60.0 };
        let path = extract(&image,[0,0,160,200],Polarity::Dark,Some(&guide),20.0).unwrap();
        assert!(path.iter().all(|p| (p[0]-66.5).abs() <= 1.0), "{path:?}");
    }

    #[test]
    fn brush_extraction_failure_returns_original_image_location_and_specific_reason() {
        for reason in ["missing_edges", "discontinuity", "ambiguous", "reversal"] {
            let mut image = FrameImage::new(100,100,vec![230;10000]);
            for y in 0..100 {
                let bands: Vec<_> = match reason {
                    "ambiguous" => vec![35..45,55..65],
                    "discontinuity" if y >= 50 => vec![55..75],
                    _ => vec![40..60],
                };
                for band in bands { for x in band { image.pixels[y*100+x]=25; } }
            }
            let center_x = if reason == "ambiguous" { 49.5 } else { 50.0 };
            let guide = BrushGuide {
                points: if reason == "reversal" { vec![[50.0,10.0],[50.0,70.0],[50.0,10.0]] } else { vec![[center_x,10.0],[center_x,90.0]] },
                radius_px: if reason == "missing_edges" { 4.0 } else { 35.0 },
            };
            let error = extract(&image,[0,0,100,100],Polarity::Dark,Some(&guide),20.0).unwrap_err();
            let ExtractionError::Failed(detail) = error else { panic!("图像提取失败必须提供可定位诊断"); };
            assert_eq!(detail.reason, reason);
            assert_eq!(detail.point[0], center_x);
            match reason {
                "missing_edges" => assert_eq!(detail.point[1],10.0),
                "discontinuity" => assert!((49.0..=53.0).contains(&detail.point[1])),
                "reversal" => assert!((67.0..=73.0).contains(&detail.point[1])),
                _ => assert!((10.0..=90.0).contains(&detail.point[1])),
            }
        }
    }
}
