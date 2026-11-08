//! 换示教件按笔对齐、局部重示教（docs/decisions/recipe-build-flow.md RB-11）。
//!
//! - 特征：每笔每台计划相机在笔内均匀取样几帧，降采样成小灰度图、去均值归一化；两笔的图像距离取各样本 1 − 相关系数的均值。
//! - 对齐：按允许插入、删除、修改的编辑距离把旧件的笔对到新件的笔；对上的两笔图像距离与帧数都接近才算“不变”，否则“改动”。
//! - 迁移：按对照把旧件每个帧组映射到新件（不变的笔按笔内偏移、改动的笔按比例），各段取映射到的帧组；新增的笔夹在同一段中间时
//!   并入该段（该段算改动），否则每笔新建一段。段的配置、走廊与示教帧跟着段走，段 ID 不变。

use std::collections::{BTreeMap, BTreeSet};

use super::burst::CameraTeaching;
use super::pieces::TeachingPiece;
use super::*;
use crate::passes;
use crate::plan::PathSegment;
use crate::recipe::SegmentCorridor;

/// 每笔每台相机取样帧数。
const SAMPLES: u32 = 6;
/// 降采样后的宽（高按原图比例）。
const THUMB_WIDTH: u32 = 40;
/// 对上的两笔图像距离不超过它才算“不变”：同一程序两件逐笔距离多在 0.1 以下，3 帧的短笔偶有 0.3 上下。
const SAME_DISTANCE: f32 = 0.3;
/// 插入或删除一笔的代价；不同笔之间的图像距离多在 0.5 以上。
const INDEL: f32 = 0.5;
/// 没有可比的样本时的图像距离：对上算改动，留给人确认。
const UNKNOWN_DISTANCE: f32 = 0.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PassChange {
    Same,
    Changed,
    Added,
    Removed,
}

/// 逐笔对照的一行：旧件的笔与新件的笔（下标从 0）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PassPair {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new: Option<u32>,
    pub change: PassChange,
    /// 两笔的图像距离（自动对照算出；人改出来的对应没有）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance: Option<f32>,
}

/// 人改的一对对应（前端回传）：旧笔与新笔下标，`changed` 为改动、否则为不变。没列出的旧笔即删除、新笔即新增。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PassLink {
    pub old: u32,
    pub new: u32,
    pub changed: bool,
}

/// 迁移后一段的沿用状态。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SegmentCarry {
    /// Same：沿用；Changed：所在笔改动（或跨了删除、并入了新增的笔）；Added：新增
    pub change: Option<PassChange>,
    /// 迁移后在新示教件上重新选过示教帧的检测相机
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub retaught: BTreeSet<String>,
    /// 沿用的段由人确认沿用
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub accepted: bool,
}

/// 迁移时去掉的旧段（显示用）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemovedSegment {
    pub index: u32,
    pub kind: crate::plan::SegmentKind,
}

/// 迁移的起点：换件前的段列表、各相机走廊、示教帧与原示教件边界。对照确认后丢弃。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AlignSource {
    pub segments: Vec<PathSegment>,
    pub corridors: BTreeMap<String, Vec<SegmentCorridor>>,
    pub teaching: Vec<CameraTeaching>,
    /// 原示教件上每段最后一个帧组
    pub cuts: Vec<u64>,
}

/// 原示教件（显示用，原图按历史记录读）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OldPiece {
    pub record_id: i64,
    pub sn: String,
    pub ts: i64,
    pub passes: Vec<u32>,
}

/// 换示教件的逐笔对照（RB-11）。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PassAlignment {
    pub old_piece: OldPiece,
    pub new_passes: Vec<u32>,
    /// 自动对照
    pub suggested: Vec<PassPair>,
    /// 当前对照（人可改）
    pub rows: Vec<PassPair>,
    pub confirmed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<AlignSource>,
    /// 按段 ID
    #[serde(default)]
    pub carry: BTreeMap<String, SegmentCarry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<RemovedSegment>,
}

// ───────────── 特征与对齐 ─────────────

/// 一笔的特征：按（相机, 取样）排列的归一化小图，读不到的为 None。
pub(super) type PassFeature = Vec<Option<Vec<f32>>>;

fn thumb(image: &crate::frame::FrameImage) -> Option<Vec<f32>> {
    let factor = (image.width / THUMB_WIDTH).max(1);
    let small = crate::frame_align::downsample(image, factor);
    let c = small.channels.max(1) as usize;
    let mut v: Vec<f32> = small
        .pixels
        .chunks(c)
        .map(|p| p.iter().map(|&x| f32::from(x)).sum::<f32>() / c as f32)
        .collect();
    let mean = v.iter().sum::<f32>() / v.len().max(1) as f32;
    v.iter_mut().for_each(|x| *x -= mean);
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (norm > 0.0).then(|| v.into_iter().map(|x| x / norm).collect())
}

/// 笔内第 i 个取样的帧组（从 1 起）。
fn sample_offset(len: u32, i: u32) -> u32 {
    (((2 * i + 1) * len) / (2 * SAMPLES)).min(len.saturating_sub(1)) + 1
}

/// 一件逐笔的特征（`path` 给出相机某帧组的原图，几台相机并行）。
pub(super) fn pass_features(
    path: &(dyn Fn(&str, u64) -> Option<std::path::PathBuf> + Sync),
    cameras: &[String],
    layout: &[u32],
) -> Vec<PassFeature> {
    let per_camera: Vec<Vec<Vec<Option<Vec<f32>>>>> = std::thread::scope(|scope| {
        let handles: Vec<_> = cameras
            .iter()
            .map(|camera| {
                scope.spawn(move || {
                    layout
                        .iter()
                        .enumerate()
                        .map(|(pass, &len)| {
                            let first = passes::first_seq(layout, pass);
                            (0..SAMPLES)
                                .map(|i| {
                                    let seq = first + u64::from(sample_offset(len, i)) - 1;
                                    thumb(&crate::image_file::load(&path(camera, seq)?).ok()?)
                                })
                                .collect()
                        })
                        .collect()
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    (0..layout.len())
        .map(|pass| {
            per_camera
                .iter()
                .flat_map(|c| c.get(pass).cloned().unwrap_or_default())
                .collect()
        })
        .collect()
}

/// 两笔的图像距离：可比样本 1 − 相关系数的均值；没有可比样本为 None。
pub(super) fn distance(a: &PassFeature, b: &PassFeature) -> Option<f32> {
    let values: Vec<f32> = a
        .iter()
        .zip(b)
        .filter_map(|(x, y)| {
            let (x, y) = (x.as_ref()?, y.as_ref()?);
            (x.len() == y.len()).then(|| 1.0 - x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>())
        })
        .collect();
    (!values.is_empty()).then(|| values.iter().sum::<f32>() / values.len() as f32)
}

/// 帧数差在这个范围内算“不变”：1 帧或一成。
fn length_close(a: u32, b: u32) -> bool {
    a.abs_diff(b) <= (a.max(b) / 10).max(1)
}

fn same(old: u32, new: u32, distance: Option<f32>) -> bool {
    length_close(old, new) && distance.is_some_and(|d| d <= SAME_DISTANCE)
}

/// 自动对照：编辑距离，对上的代价为图像距离加帧数相对差，插入、删除各 `INDEL`。
pub(super) fn align(old: &[u32], new: &[u32], distances: &[Vec<Option<f32>>]) -> Vec<PassPair> {
    let (n, m) = (old.len(), new.len());
    let cost = |i: usize, j: usize| {
        let d = distances[i][j].unwrap_or(UNKNOWN_DISTANCE);
        d + old[i].abs_diff(new[j]) as f32 / old[i].max(new[j]).max(1) as f32
    };
    let mut table = vec![vec![f32::INFINITY; m + 1]; n + 1];
    table[0][0] = 0.0;
    for i in 0..=n {
        for j in 0..=m {
            let mut best = table[i][j];
            if i > 0 && j > 0 {
                best = best.min(table[i - 1][j - 1] + cost(i - 1, j - 1));
            }
            if i > 0 {
                best = best.min(table[i - 1][j] + INDEL);
            }
            if j > 0 {
                best = best.min(table[i][j - 1] + INDEL);
            }
            table[i][j] = best;
        }
    }
    let mut rows = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        if i > 0 && j > 0 && table[i][j] == table[i - 1][j - 1] + cost(i - 1, j - 1) {
            let d = distances[i - 1][j - 1];
            rows.push(PassPair {
                old: Some(i as u32 - 1),
                new: Some(j as u32 - 1),
                change: if same(old[i - 1], new[j - 1], d) {
                    PassChange::Same
                } else {
                    PassChange::Changed
                },
                distance: d,
            });
            (i, j) = (i - 1, j - 1);
        } else if i > 0 && table[i][j] == table[i - 1][j] + INDEL {
            rows.push(removed(i as u32 - 1));
            i -= 1;
        } else {
            rows.push(added(j as u32 - 1));
            j -= 1;
        }
    }
    rows.reverse();
    rows
}

fn removed(old: u32) -> PassPair {
    PassPair {
        old: Some(old),
        new: None,
        change: PassChange::Removed,
        distance: None,
    }
}

fn added(new: u32) -> PassPair {
    PassPair {
        old: None,
        new: Some(new),
        change: PassChange::Added,
        distance: None,
    }
}

/// 人改的对应换成对照：对应须保持两件笔的先后顺序；没列出的旧笔为删除、新笔为新增，夹在相邻两对对应之间（删除在前）。
/// 与自动对照同一对的沿用它的图像距离。
pub(super) fn rows_from_links(
    links: &[PassLink],
    old: usize,
    new: usize,
    suggested: &[PassPair],
) -> Result<Vec<PassPair>, String> {
    let mut rows = Vec::new();
    let (mut i, mut j) = (0u32, 0u32);
    for link in links {
        if link.old as usize >= old || link.new as usize >= new {
            return Err(format!(
                "对照里旧件第 {} 笔或新件第 {} 笔不存在",
                link.old + 1,
                link.new + 1
            ));
        }
        if link.old < i || link.new < j {
            return Err("逐笔对照要保持两件笔的先后顺序，每一笔至多对一次".into());
        }
        rows.extend((i..link.old).map(removed));
        rows.extend((j..link.new).map(added));
        let distance = suggested
            .iter()
            .find(|r| r.old == Some(link.old) && r.new == Some(link.new))
            .and_then(|r| r.distance);
        rows.push(PassPair {
            old: Some(link.old),
            new: Some(link.new),
            change: if link.changed {
                PassChange::Changed
            } else {
                PassChange::Same
            },
            distance,
        });
        (i, j) = (link.old + 1, link.new + 1);
    }
    rows.extend((i..old as u32).map(removed));
    rows.extend((j..new as u32).map(added));
    Ok(rows)
}

// ───────────── 迁移 ─────────────

/// 按对照迁移后的段列表、走廊、示教帧（帧组号已在新件上，轮次由调用方改）、新件上的边界与各段沿用状态。
#[derive(Debug)]
pub(super) struct Migrated {
    pub segments: Vec<PathSegment>,
    pub corridors: BTreeMap<String, Vec<SegmentCorridor>>,
    pub teaching: Vec<CameraTeaching>,
    pub cuts: Vec<u64>,
    pub carry: BTreeMap<String, SegmentCarry>,
    pub removed: Vec<RemovedSegment>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Owner {
    None,
    Old(usize),
    Added,
}

/// 旧件每个帧组（下标 = seq − 1）在新件上的帧组；所在笔删除为 None。
fn frame_map(old: &[u32], new: &[u32], rows: &[PassPair]) -> Vec<Option<u64>> {
    let total: u64 = old.iter().map(|&n| u64::from(n)).sum();
    let mut map = vec![None; total as usize];
    for row in rows {
        let (Some(p), Some(q)) = (row.old, row.new) else {
            continue;
        };
        let (p, q) = (p as usize, q as usize);
        let (len_p, len_q) = (u64::from(old[p]), u64::from(new[q]));
        let (from, to) = (passes::first_seq(old, p), passes::first_seq(new, q));
        for o in 1..=len_p {
            let mapped = match row.change {
                PassChange::Same => o.min(len_q),
                // 按比例、对齐笔首：每笔第一帧对到新笔第一帧，空出的帧组归前一个旧段（即同一笔里的段）
                _ => (o - 1) * len_q / len_p + 1,
            };
            map[(from + o - 2) as usize] = Some(to + mapped - 1);
        }
    }
    map
}

/// 按对照迁移（见模块说明）。`source.segments` 须已有段 ID；`cuts` 与旧件笔结构对得上。
pub(super) fn migrate(
    source: &AlignSource,
    old: &[u32],
    new: &[u32],
    rows: &[PassPair],
) -> Migrated {
    let old_total: u64 = old.iter().map(|&n| u64::from(n)).sum();
    let new_total: u64 = new.iter().map(|&n| u64::from(n)).sum();
    let ends: Vec<u64> = source.cuts.iter().copied().chain([old_total]).collect();
    let segment_of = |seq: u64| {
        ends.iter()
            .position(|&end| seq <= end)
            .unwrap_or(ends.len() - 1)
    };
    let map = frame_map(old, new, rows);
    let change_of_old_pass: BTreeMap<usize, PassChange> = rows
        .iter()
        .filter_map(|r| Some((r.old? as usize, r.change)))
        .collect();
    // 旧段是否全部落在不变的笔里且没丢帧
    let mut intact = vec![true; source.segments.len()];
    let mut owner = vec![Owner::None; new_total as usize];
    for seq in 1..=old_total {
        let k = segment_of(seq);
        let pass = passes::locate(old, seq).map(|(p, _)| p);
        if pass.and_then(|p| change_of_old_pass.get(&p)) != Some(&PassChange::Same) {
            intact[k] = false;
        }
        if let Some(x) = map[(seq - 1) as usize] {
            owner[(x - 1) as usize] = Owner::Old(k);
        }
    }
    let added_passes: Vec<(u64, u64)> = rows
        .iter()
        .filter(|r| r.old.is_none())
        .filter_map(|r| r.new)
        .map(|q| {
            let first = passes::first_seq(new, q as usize);
            (first, first + u64::from(new[q as usize]) - 1)
        })
        .collect();
    for &(first, last) in &added_passes {
        for x in first..=last {
            owner[(x - 1) as usize] = Owner::Added;
        }
    }
    // 不变或改动的笔里没映射到的帧组归前一个旧段（开头没有就归后一个）
    let mut previous = None;
    for x in 0..owner.len() {
        match owner[x] {
            Owner::Old(k) => previous = Some(k),
            Owner::None => {
                owner[x] = match previous.or_else(|| {
                    owner[x..].iter().find_map(|o| match o {
                        Owner::Old(k) => Some(*k),
                        _ => None,
                    })
                }) {
                    Some(k) => Owner::Old(k),
                    None => Owner::Added,
                }
            }
            Owner::Added => {}
        }
    }
    // 新增的笔夹在同一旧段中间时并入该段
    for &(first, last) in &added_passes {
        let before = (1..first)
            .rev()
            .find_map(|x| match owner[(x - 1) as usize] {
                Owner::Old(k) => Some(k),
                _ => None,
            });
        let after = (last + 1..=new_total).find_map(|x| match owner[(x - 1) as usize] {
            Owner::Old(k) => Some(k),
            _ => None,
        });
        if let (Some(a), Some(b)) = (before, after) {
            if a == b {
                for x in first..=last {
                    owner[(x - 1) as usize] = Owner::Old(a);
                }
                intact[a] = false;
            }
        }
    }
    // 连续同主的帧组成段；新增的笔每笔一段
    let mut runs: Vec<(Owner, u64, u64)> = Vec::new();
    for x in 1..=new_total {
        let o = owner[(x - 1) as usize];
        let pass_start = added_passes.iter().any(|&(a, _)| a == x);
        match runs.last_mut() {
            Some(run) if run.0 == o && !(o == Owner::Added && pass_start) => run.2 = x,
            _ => runs.push((o, x, x)),
        }
    }
    let mut segments: Vec<PathSegment> = Vec::new();
    let mut changes: Vec<PassChange> = Vec::new();
    let mut placed: BTreeMap<usize, usize> = BTreeMap::new();
    for (r, &(o, _, _)) in runs.iter().enumerate() {
        let (segment, change) = match o {
            Owner::Old(k) if !placed.contains_key(&k) => {
                placed.insert(k, r);
                let mut segment = source.segments[k].clone();
                let change = if intact[k] {
                    PassChange::Same
                } else {
                    segment.length_mm = None;
                    PassChange::Changed
                };
                (segment, change)
            }
            Owner::Old(k) => (fresh(&source.segments[k]), PassChange::Changed),
            _ => {
                let neighbor = runs[..r]
                    .iter()
                    .rev()
                    .chain(runs[r + 1..].iter())
                    .find_map(|run| match run.0 {
                        Owner::Old(k) => Some(k),
                        _ => None,
                    })
                    .unwrap_or(0);
                (fresh(&source.segments[neighbor]), PassChange::Added)
            }
        };
        segments.push(segment);
        changes.push(change);
    }
    for (i, segment) in segments.iter_mut().enumerate() {
        segment.index = i as u32 + 1;
    }
    crate::plan::ensure_segment_ids(&mut segments);
    let new_index = |k: usize| placed.get(&k).map(|&r| r as u32 + 1);
    let corridors = source
        .corridors
        .iter()
        .map(|(camera, list)| {
            let mut list: Vec<SegmentCorridor> = list
                .iter()
                .filter_map(|c| {
                    let k = source.segments.iter().position(|s| s.index == c.segment)?;
                    Some(SegmentCorridor {
                        segment: new_index(k)?,
                        ..c.clone()
                    })
                })
                .collect();
            list.sort_by_key(|c| c.segment);
            (camera.clone(), list)
        })
        .collect();
    let teaching = source
        .teaching
        .iter()
        .filter_map(|t| {
            let k = source
                .segments
                .iter()
                .position(|s| Some(s.index) == t.segment)?;
            let r = *placed.get(&k)?;
            let (_, first, last) = runs[r];
            let seq = t
                .seq
                .checked_sub(1)
                .and_then(|i| map.get(i as usize).copied().flatten())
                .filter(|x| (first..=last).contains(x))
                .unwrap_or((first + last) / 2);
            Some(CameraTeaching {
                seq,
                segment: Some(r as u32 + 1),
                ..t.clone()
            })
        })
        .collect();
    let removed = source
        .segments
        .iter()
        .enumerate()
        .filter(|(k, _)| !placed.contains_key(k))
        .map(|(_, s)| RemovedSegment {
            index: s.index,
            kind: s.kind,
        })
        .collect();
    let carry = segments
        .iter()
        .zip(&changes)
        .filter_map(|(s, &change)| {
            Some((
                s.id.clone()?,
                SegmentCarry {
                    change: Some(change),
                    ..SegmentCarry::default()
                },
            ))
        })
        .collect();
    let cuts = runs[..runs.len().saturating_sub(1)]
        .iter()
        .map(|run| run.2)
        .collect();
    Migrated {
        segments,
        corridors,
        teaching,
        cuts,
        carry,
        removed,
    }
}

/// 新建一段：配置取 `like`，去掉段 ID、帧数、段长、标定、胶偏参考值与段首尾跳过。
fn fresh(like: &PathSegment) -> PathSegment {
    let mut segment = like.clone();
    segment.id = None;
    segment.expected_frames = None;
    segment.length_mm = None;
    segment.calib = None;
    segment.skip_head = 0;
    segment.skip_tail = 0;
    if let Some(reference) = segment.offset_ref.as_mut() {
        (reference.x_ref, reference.spread) = (None, None);
    }
    segment
}

// ───────────── 候选上的操作 ─────────────

/// 换示教件时自动对照所需：原示教件（笔结构已补齐）、原边界与逐笔图像距离。
pub(super) struct AlignInput {
    pub old_piece: TeachingPiece,
    pub cuts: Vec<u64>,
    pub distances: Vec<Vec<Option<f32>>>,
}

/// 把迁移结果写进候选：段列表、走廊、示教帧（改指 `round`）；清空边界复核与示教件测量（同 RB-2）。返回新件上的边界草稿。
fn apply(w: &mut Workspace, migrated: Migrated, round: &str) -> Result<Vec<u64>, String> {
    let before = w.doc.clone();
    w.doc.segments = migrated.segments;
    for inspect in &mut w.doc.inspect {
        inspect.segments = migrated
            .corridors
            .get(&inspect.camera)
            .cloned()
            .unwrap_or_default();
    }
    super::burst::clear_calibration(&mut w.doc);
    w.doc.build()?;
    if navigation::structure_changed(&before, &w.doc) {
        w.navigation_structure_revision += 1;
    }
    w.camera_teaching = migrated
        .teaching
        .into_iter()
        .map(|t| CameraTeaching {
            round: round.to_string(),
            ..t
        })
        .collect();
    w.capture_review = capture_review::CaptureReview::default();
    w.calibration = super::burst::Calibration::default();
    if let Some(alignment) = w.pass_alignment.as_mut() {
        alignment.carry = migrated.carry;
        alignment.removed = migrated.removed;
    }
    Ok(migrated.cuts)
}

/// 原示教件的边界能对上它的笔结构：边界递增且都在帧组数以内。
pub(super) fn cuts_fit(cuts: &[u64], passes: &[u32]) -> bool {
    let total: u64 = passes.iter().map(|&n| u64::from(n)).sum();
    !passes.is_empty()
        && cuts.windows(2).all(|w| w[0] < w[1])
        && cuts.iter().all(|&c| c >= 1 && c < total)
}

/// 换示教件（RB-11）：按自动对照迁移示教，对照待人确认。返回新件上的边界草稿，由调用方保存为新件的边界草稿。
pub(super) fn replace_by_passes(
    w: &mut Workspace,
    piece: TeachingPiece,
    input: AlignInput,
) -> Result<Vec<u64>, String> {
    super::burst::require_burst(&w.doc)?;
    crate::plan::ensure_segment_ids(&mut w.doc.segments);
    let source = AlignSource {
        segments: w.doc.segments.clone(),
        corridors: w
            .doc
            .inspect
            .iter()
            .map(|i| (i.camera.clone(), i.segments.clone()))
            .collect(),
        teaching: w.camera_teaching.clone(),
        cuts: input.cuts,
    };
    let old = &input.old_piece;
    let suggested = align(&old.passes, &piece.passes, &input.distances);
    let migrated = migrate(&source, &old.passes, &piece.passes, &suggested);
    w.pass_alignment = Some(PassAlignment {
        old_piece: OldPiece {
            record_id: old.record_id,
            sn: old.sn.clone(),
            ts: old.ts,
            passes: old.passes.clone(),
        },
        new_passes: piece.passes.clone(),
        rows: suggested.clone(),
        suggested,
        confirmed: false,
        source: Some(source),
        carry: BTreeMap::new(),
        removed: Vec::new(),
    });
    let round = piece.round_id.clone();
    w.teaching_piece = Some(piece);
    let cuts = apply(w, migrated, &round)?;
    w.changed();
    Ok(cuts)
}

/// 改逐笔对照并（可选）确认：未确认前按新对照从迁移起点重新迁移，确认后丢弃起点、对照不再能改。返回需要保存的边界草稿
/// （对照没变时为 None）。
pub(super) fn edit_alignment(
    w: &mut Workspace,
    links: &[PassLink],
    confirm: bool,
) -> Result<Option<Vec<u64>>, String> {
    let alignment = w.pass_alignment.as_ref().ok_or("没有待确认的逐笔对照")?;
    if alignment.confirmed {
        return Err("逐笔对照已确认".into());
    }
    let source = alignment.source.clone().ok_or("逐笔对照缺迁移起点")?;
    let rows = rows_from_links(
        links,
        alignment.old_piece.passes.len(),
        alignment.new_passes.len(),
        &alignment.suggested,
    )?;
    let mut cuts = None;
    if rows != alignment.rows {
        let migrated = migrate(
            &source,
            &alignment.old_piece.passes,
            &alignment.new_passes,
            &rows,
        );
        let round = w
            .teaching_piece
            .as_ref()
            .ok_or("先选示教件")?
            .round_id
            .clone();
        if let Some(alignment) = w.pass_alignment.as_mut() {
            alignment.rows = rows;
        }
        cuts = Some(apply(w, migrated, &round)?);
    }
    if confirm {
        if let Some(alignment) = w.pass_alignment.as_mut() {
            alignment.confirmed = true;
            alignment.source = None;
        }
    }
    w.changed();
    Ok(cuts)
}

/// 对照未确认前不能改段列表、不能保存分段。
pub(super) fn require_confirmed(w: &Workspace) -> Result<(), String> {
    if w.pass_alignment.as_ref().is_some_and(|a| !a.confirmed) {
        return Err("换示教件的逐笔对照还没确认：先在分段页确认对照".into());
    }
    Ok(())
}

/// 在新示教件上重新选了示教帧：记下这台相机。
pub(super) fn mark_retaught(w: &mut Workspace, segment: u32, camera: &str) {
    let Some(alignment) = w.pass_alignment.as_mut() else {
        return;
    };
    let Some(id) = w
        .doc
        .segments
        .iter()
        .find(|s| s.index == segment)
        .and_then(|s| s.id.clone())
    else {
        return;
    };
    alignment
        .carry
        .entry(id)
        .or_default()
        .retaught
        .insert(camera.to_string());
}

/// 人确认（或撤销确认）沿用某一段：只对沿用的段。
pub(super) fn accept_carry(w: &mut Workspace, segment: u32, accepted: bool) -> Result<(), String> {
    let id = w
        .doc
        .segments
        .iter()
        .find(|s| s.index == segment)
        .ok_or_else(|| format!("没有第 {segment} 段"))?
        .id
        .clone();
    let carry = w
        .pass_alignment
        .as_mut()
        .and_then(|a| a.carry.get_mut(id.as_deref()?))
        .filter(|c| c.change == Some(PassChange::Same))
        .ok_or_else(|| format!("第 {segment} 段不是沿用的段"))?;
    carry.accepted = accepted;
    w.changed();
    Ok(())
}

/// 一段还要人做什么（发布前检查）：None 为已处理。
pub(super) fn pending_work(w: &Workspace, segment: &PathSegment) -> Option<String> {
    let alignment = w.pass_alignment.as_ref()?;
    let cameras = crate::plan::segment_cameras(segment, &w.doc.plan_cameras());
    if cameras.is_empty() {
        return None;
    }
    let carry = segment.id.as_ref().and_then(|id| alignment.carry.get(id));
    let missing: Vec<&str> = cameras
        .iter()
        .filter(|c| !carry.is_some_and(|carry| carry.retaught.contains(*c)))
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        return None;
    }
    match carry.and_then(|c| c.change) {
        Some(PassChange::Same) if carry.is_some_and(|c| c.accepted) => None,
        Some(PassChange::Same) => Some(format!("第 {} 段沿用，待确认沿用", segment.index)),
        Some(PassChange::Changed) => Some(format!(
            "第 {} 段所在笔改动，相机 {} 未在新件上重新示教",
            segment.index,
            missing.join("、")
        )),
        _ => Some(format!(
            "第 {} 段为新增，相机 {} 未示教",
            segment.index,
            missing.join("、")
        )),
    }
}

/// 发布前检查“按笔对照”：没有对照时不列。
pub(super) fn alignment_check(w: &Workspace) -> Option<Check> {
    let alignment = w.pass_alignment.as_ref()?;
    let count = |change: PassChange| alignment.rows.iter().filter(|r| r.change == change).count();
    let summary = format!(
        "沿用 {} 笔、改动 {} 笔、新增 {} 笔、删除 {} 笔",
        count(PassChange::Same),
        count(PassChange::Changed),
        count(PassChange::Added),
        count(PassChange::Removed)
    );
    if !alignment.confirmed {
        return Some(Check {
            advisory: false,
            name: "按笔对照".into(),
            passed: false,
            detail: format!("{summary}；逐笔对照还没确认：先在分段页确认"),
        });
    }
    let pending: Vec<String> = w
        .doc
        .segments
        .iter()
        .filter_map(|s| pending_work(w, s))
        .collect();
    Some(Check {
        advisory: false,
        name: "按笔对照".into(),
        passed: pending.is_empty(),
        detail: if pending.is_empty() {
            format!("{summary}；改动与新增的段已重新示教，沿用的段已确认")
        } else {
            format!("{summary}；{}", pending.join("；"))
        },
    })
}

// ───────────── 命令 ─────────────

/// 改逐笔对照（未确认前），`confirm` 时同时确认。对照变了按新对照重新迁移，并把新边界存为示教件的边界草稿。
#[tauri::command]
pub async fn workspace_pass_alignment_save(
    app: AppHandle,
    id: String,
    revision: u64,
    links: Vec<PassLink>,
    confirm: bool,
) -> Result<WorkspaceView, String> {
    let cycle = app.state::<CycleHost>();
    let _config = cycle.config_gate.lock().await;
    let task = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let host = task.state::<WorkspaceHost>();
        let cycle = task.state::<CycleHost>();
        let capture = cycle.camera.recipe_capture();
        let mut items = host.items.lock().unwrap();
        let current = items.get_mut(&id).ok_or("候选配置不存在")?;
        current.expect(revision)?;
        let mut next = current.clone();
        if let Some(cuts) = edit_alignment(&mut next, &links, confirm)? {
            let round_id = next
                .teaching_piece
                .as_ref()
                .ok_or("先选示教件")?
                .round_id
                .clone();
            let round = capture.get_burst(&round_id)?;
            capture_review::apply_capture_review(
                &mut next,
                &round,
                capture_review::CaptureReviewEdit::Save(cuts),
            )?;
        }
        host.save(&next)?;
        *current = next.clone();
        drop(items);
        view(&task, next)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// 确认（或撤销确认）沿用某一段（RB-11：沿用的段在新件上试测通过或由人确认沿用）。
#[tauri::command]
pub fn workspace_pass_carry_accept(
    app: AppHandle,
    id: String,
    revision: u64,
    segment: u32,
    accepted: bool,
) -> Result<WorkspaceView, String> {
    let host = app.state::<WorkspaceHost>();
    let mut items = host.items.lock().unwrap();
    let current = items.get_mut(&id).ok_or("候选配置不存在")?;
    current.expect(revision)?;
    let mut next = current.clone();
    accept_carry(&mut next, segment, accepted)?;
    host.save(&next)?;
    *current = next.clone();
    drop(items);
    view(&app, next)
}
