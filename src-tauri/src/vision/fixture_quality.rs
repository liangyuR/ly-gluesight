//! 特征定位的示教检查与验证统计（定稿 docs/discussions/fixture-position/final.md §5“示教”“验证”、§10、§13）。
//! 纯函数：不做 IO、不跑引擎。示教页的保存门（`geometry_checks` + `teach_acceptance`）与验证页的统计
//! （`validation_report`）都在这里，门限的出处写在常量上。

use serde::{Deserialize, Serialize};

use super::fixture::padded_rect;
use super::taught::{judged_stations, PoseRecord, MIN_JUDGED_STATIONS};
use crate::frame::FrameImage;
use crate::recipe::{Clearance, FixtureSpec, FIXTURE_ANGLE_STEP_DEG};

/// 模板（内框）灰度标准差下限：与阶段 0 评测选框一致（`eval/locate.rs` 的 MIN_STD）
pub const MIN_STD: f64 = 12.0;
/// 亮像素：灰度 ≥ 215 的像素在整图 +40 时会饱和（`eval/locate.rs` 的 BRIGHT_LEVEL）
pub const BRIGHT_LEVEL: u8 = 215;
/// 亮像素比例上限（定稿 §10：示教保存时的预筛，必须过的是验证时的亮度 ±40 试定位）
pub const MAX_BRIGHT_FRACTION: f64 = 0.1;
/// ±40 下的锁定判据：与原图位姿的模板中心差 ≤ 2 px、角度差 ≤ 0.3° 算锁对，否则算错锁（评测的 LOCK_TOL_PX / LOCK_TOL_DEG）
pub const LOCK_TOL_PX: f64 = 2.0;
pub const LOCK_TOL_DEG: f64 = 0.3;
/// 示教图自定位的恒等容差：模板就是从这张图裁的，位姿应为恒等。平移 0.5 px；开转角时角度不超过半个扫描步长
pub const IDENTITY_TOL_PX: f64 = 0.5;
pub const IDENTITY_TOL_DEG: f64 = FIXTURE_ANGLE_STEP_DEG as f64 / 2.0;
/// 位姿单轴超过搜索半径的这个比例告警（定稿 §5“验证”）
pub const SEARCH_WARN: f64 = 0.7;
/// 验证时的整图亮度扰动（定稿 §10：每个框都要过亮度 ±40 的离线试定位）
pub const BRIGHTNESS_PROBE: f64 = 40.0;
/// 转角证据的离线扫描范围 ±（°，定稿 §10 决定二）
pub const ROTATION_PROBE_DEG: f32 = 3.0;
/// 杠杆臂告警：未补偿的 0.25° 转动在最远站造成的误差超过位置公差的 1/5 时告警（公差 ±2 mm 时 L ≈ 92 mm，
/// 对应决定二里 L = 87 mm、0.26° 的量级）
pub const LEVER_REFERENCE_DEG: f64 = 0.25;
/// 少于这么多件独立正常样本时，P99 就是最大值，提示样本不足
pub const FEW_SAMPLES: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level { Block, Warn }

/// 检查项编号。前端按编号显示，不解析说明文字。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CheckId {
    // 示教图检查（保存门）
    Inside, Corridor, Texture, Bright, Lever, SelfLocate,
    // 验证统计（发布门）
    LocateAll, Sigma, BrightnessLock, Observable, SearchRange, Nominal, BrightnessReject, Rotation, EndParams, FewSamples,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckItem {
    pub id: CheckId,
    pub level: Level,
    pub passed: bool,
    pub detail: String,
}

impl CheckItem {
    fn new(id: CheckId, level: Level, passed: bool, detail: String) -> Self { Self { id, level, passed, detail } }
    pub fn blocks(&self) -> bool { self.level == Level::Block && !self.passed }
}

/// 第一项没过的阻断项（试测无效、保存或发布被拦的原因）。
pub fn first_block(items: &[CheckItem]) -> Option<&CheckItem> {
    items.iter().find(|c| c.blocks())
}

/// glue.locate 的 reason（及 `angle_unobservable`）译成中文。唯一出处，不写测试。
pub fn reason_text(reason: Option<&str>) -> String {
    match reason {
        None | Some("") => "定位失败".into(),
        Some("low_score") => "匹配分数低于最低分数".into(),
        Some("ambiguous") => "有相近的次峰（次峰比超过上限，模板不唯一）".into(),
        Some("at_search_edge") => "峰贴到搜索范围边上（偏移超出搜索半径）".into(),
        Some("at_angle_edge") => "角度贴到扫描边界".into(),
        Some("flat_template") => "模板纹理太弱".into(),
        Some("out_of_image") => "模板或搜索区超出图像".into(),
        Some("angle_unobservable") => "开了转角但看不出角度".into(),
        Some(other) => format!("定位失败（{other}）"),
    }
}

/// 内框的灰度标准差与亮像素比例（口径同评测选框：只算内框，不含外圈）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TemplateStats { pub std: f64, pub bright: f64 }

pub fn template_stats(img: &FrameImage, rect: [u32; 4]) -> Option<TemplateStats> {
    let [x, y, w, h] = rect;
    if w == 0 || h == 0 || x.checked_add(w)? > img.width || y.checked_add(h)? > img.height
        || img.pixels.len() as u64 != u64::from(img.width) * u64::from(img.height) {
        return None;
    }
    let (mut sum, mut sq, mut bright) = (0.0f64, 0.0f64, 0usize);
    for row in y..y + h {
        let start = (row * img.width + x) as usize;
        for &v in &img.pixels[start..start + w as usize] {
            sum += f64::from(v);
            sq += f64::from(v) * f64::from(v);
            bright += usize::from(v >= BRIGHT_LEVEL);
        }
    }
    let n = f64::from(w) * f64::from(h);
    let mean = sum / n;
    Some(TemplateStats { std: (sq / n - mean * mean).max(0.0).sqrt(), bright: bright as f64 / n })
}

/// 整图加 delta 后截断到 0–255（同评测 `synth::Perturb::Brightness`）。
pub fn brightness(img: &FrameImage, delta: f64) -> FrameImage {
    FrameImage::new(img.width, img.height, img.pixels.iter().map(|&v| (f64::from(v) + delta).round().clamp(0.0, 255.0) as u8).collect())
}

/// 杠杆臂 L：示教框中心到最远站的距离。折线上离一点最远的点必在顶点上，所以只看中线顶点。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lever {
    pub from: [f64; 2],
    pub to: [f32; 2],
    pub px: f64,
    pub mm: Option<f64>,
}

pub fn lever(fixture: &FixtureSpec, path: &[[f32; 2]], mm_per_px: Option<f64>) -> Option<Lever> {
    let from = fixture.teach_center();
    let to = *path.iter().max_by(|a, b| {
        let d = |p: &[f32; 2]| (f64::from(p[0]) - from[0]).hypot(f64::from(p[1]) - from[1]);
        d(a).total_cmp(&d(b))
    })?;
    let px = (f64::from(to[0]) - from[0]).hypot(f64::from(to[1]) - from[1]);
    Some(Lever { from, to, px, mm: mm_per_px.map(|m| px * m) })
}

/// 杠杆臂 L 上 0.25° 未补偿转动造成的误差（mm）超过公差/5 时告警的 L（mm）。
pub fn lever_limit_mm(tolerance_mm: f64) -> f64 {
    tolerance_mm / 5.0 / LEVER_REFERENCE_DEG.to_radians().tan()
}

/// 示教图检查的输入：草稿或已保存的定位框、它所在的冻结示教原图、本框到同组各条走廊的余量。
pub struct TeachInput<'a> {
    pub fixture: &'a FixtureSpec,
    pub image: &'a FrameImage,
    /// `RecipeDoc::fixture_clearances` 里属于这个框的项
    pub clearances: &'a [Clearance],
    /// 本视图示教中线（算杠杆臂）
    pub path: &'a [[f32; 2]],
    pub mm_per_px: Option<f64>,
    /// 位置公差 min(上公差, 下公差)（mm）；没设位置限值时为空，杠杆臂不评估
    pub tolerance_mm: Option<f64>,
}

/// 示教图检查结果（不跑引擎的部分），也是 `workspace_fixture_check` 的返回值：画框时实时显示。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureCheck {
    pub passed: bool,
    pub checks: Vec<CheckItem>,
    /// 带外圈的模板框（出图时为空）
    pub padded: Option<[u32; 4]>,
    /// 模板框能移到的范围：示教框四周各扩 searchRadius（px）
    pub search: [f64; 4],
    pub lever: Option<Lever>,
    pub texture_std: Option<f64>,
    pub bright_fraction: Option<f64>,
}

pub fn geometry_checks(input: &TeachInput) -> FixtureCheck {
    let f = input.fixture;
    let [x, y, w, h] = f.rect;
    let r = f64::from(f.search_radius_px);
    let padded = padded_rect(f.rect, input.image.width, input.image.height);
    let mut checks = vec![CheckItem::new(CheckId::Inside, Level::Block, padded.is_some(), match padded {
        Some(_) => "带 4 px 外圈的模板框在示教图内".into(),
        None => format!("定位框加 4 px 外圈超出示教图（{}×{}），请把框从图边往里收", input.image.width, input.image.height),
    })];
    let tightest = input.clearances.iter().min_by(|a, b| (a.clearance - a.need).total_cmp(&(b.clearance - b.need)));
    checks.push(CheckItem::new(CheckId::Corridor, Level::Block, input.clearances.iter().all(Clearance::passed), match tightest {
        Some(c) if !c.passed() => c.message(),
        Some(c) => format!("离最近的胶检测走廊（{}）{:.1} px，需要大于 {:.1} px", c.owner, c.clearance, c.need),
        None => "同一场景还没有已示教的中线".into(),
    }));
    let stats = template_stats(input.image, f.rect);
    checks.push(CheckItem::new(CheckId::Texture, Level::Block, stats.is_some_and(|s| s.std >= MIN_STD), match stats {
        Some(s) if s.std >= MIN_STD => format!("框内灰度标准差 {:.1}（≥ {MIN_STD}）", s.std),
        Some(s) => format!("框内纹理不足：灰度标准差 {:.1}，需要 ≥ {MIN_STD}；请框孔、角点等有明显结构的特征", s.std),
        None => "框超出图像，无法计算纹理".into(),
    }));
    checks.push(CheckItem::new(CheckId::Bright, Level::Block, stats.is_some_and(|s| s.bright < MAX_BRIGHT_FRACTION), match stats {
        Some(s) if s.bright < MAX_BRIGHT_FRACTION => format!("亮像素（≥ {BRIGHT_LEVEL}）占 {:.1}%（< {:.0}%）", s.bright * 100.0, MAX_BRIGHT_FRACTION * 100.0),
        Some(s) => format!("亮像素（≥ {BRIGHT_LEVEL}）占 {:.1}%，需要 < {:.0}%：镜面高光、过曝区在亮度变化时容易错锁，请换框", s.bright * 100.0, MAX_BRIGHT_FRACTION * 100.0),
        None => "框超出图像，无法计算亮像素".into(),
    }));
    let lever = lever(f, input.path, input.mm_per_px);
    let lever_item = match (lever.as_ref().and_then(|l| l.mm), input.tolerance_mm) {
        (Some(mm), Some(tol)) => {
            let limit = lever_limit_mm(tol);
            CheckItem::new(CheckId::Lever, Level::Warn, mm <= limit, if mm <= limit {
                format!("杠杆臂 L = {mm:.1} mm（≤ {limit:.0} mm）")
            } else {
                format!("杠杆臂 L = {mm:.1} mm，超过 {limit:.0} mm：只找平移时 {LEVER_REFERENCE_DEG}° 的转动在最远站就超过位置公差的 1/5，尽量选离胶路近的特征")
            })
        }
        _ => CheckItem::new(CheckId::Lever, Level::Warn, true, "还没有中线、像素当量或位置限值，杠杆臂暂不评估".into()),
    };
    checks.push(lever_item);
    FixtureCheck {
        passed: first_block(&checks).is_none(),
        checks,
        padded,
        search: [f64::from(x) - r, f64::from(y) - r, f64::from(w) + 2.0 * r, f64::from(h) + 2.0 * r],
        lever,
        texture_std: stats.map(|s| s.std),
        bright_fraction: stats.map(|s| s.bright),
    }
}

/// 示教图检查报告：几何与纹理检查加示教图上的定位验收，存进试测记录（`Trial.fixture`）。
/// 试测有效 = 胶路判定可用且这份报告通过（定稿 §13 的定位验收定义）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureTeachReport {
    pub passed: bool,
    pub checks: Vec<CheckItem>,
    pub pose: Option<PoseRecord>,
    pub lever: Option<Lever>,
    pub texture_std: Option<f64>,
    pub bright_fraction: Option<f64>,
}

/// 加上示教图自定位（`selfLocate`）：位姿必须被采用（含最低分数、次峰比门，开转角时还要看得出角度），
/// 且接近恒等（偏移 ≤ 0.5 px，开转角时角度 ≤ 半个扫描步长）。`pose` 为空表示示教图上没跑出定位结果。
pub fn teach_acceptance(geometry: FixtureCheck, pose: Option<&PoseRecord>, angle_range_deg: f32, max_peak_ratio: f32) -> FixtureTeachReport {
    let mut checks = geometry.checks;
    let item = match pose {
        None => CheckItem::new(CheckId::SelfLocate, Level::Block, false, "示教图上没有得到定位结果".into()),
        Some(p) if !p.accepted => CheckItem::new(CheckId::SelfLocate, Level::Block, false,
            format!("示教图上定位没被采用：{}{}", reason_text(p.reason.as_deref()), p.peak_ratio.map(|r| format!("（次峰比 {r:.2}，上限 {max_peak_ratio:.2}）")).unwrap_or_default())),
        Some(p) => {
            let offset = p.offset.map(|o| f64::from(o[0]).hypot(f64::from(o[1])));
            let angle = p.angle.map(|a| f64::from(a).abs()).unwrap_or(0.0);
            let still = offset.is_some_and(|o| o <= IDENTITY_TOL_PX) && (angle_range_deg == 0.0 || angle <= IDENTITY_TOL_DEG);
            let summary = format!("偏移 {:.2} px、角度 {:.2}°、分数 {}、次峰比 {}（上限 {max_peak_ratio:.2}）",
                offset.unwrap_or(f64::NAN), angle, p.score.map_or("—".into(), |s| format!("{s:.3}")), p.peak_ratio.map_or("—".into(), |r| format!("{r:.2}")));
            if still {
                CheckItem::new(CheckId::SelfLocate, Level::Block, true, format!("示教图上定位被采用：{summary}"))
            } else {
                CheckItem::new(CheckId::SelfLocate, Level::Block, false, format!("示教图上自定位不是恒等（{summary}；应偏移 ≤ {IDENTITY_TOL_PX} px{}）：模板可能不唯一或含会动的结构",
                    if angle_range_deg > 0.0 { format!("、角度 ≤ {IDENTITY_TOL_DEG}°") } else { String::new() }))
            }
        }
    };
    checks.push(item);
    FixtureTeachReport { passed: first_block(&checks).is_none(), checks, pose: pose.cloned(), lever: geometry.lever, texture_std: geometry.texture_std, bright_fraction: geometry.bright_fraction }
}

/// 一个特征定位视图的验证背景：门限、公差、杠杆臂与站表。
#[derive(Clone, Debug)]
pub struct ViewContext {
    pub k: usize,
    pub view: u8,
    pub shot_id: String,
    pub fixture: FixtureSpec,
    /// 位置公差 min(上公差, 下公差)（mm）与名义值
    pub tolerance_mm: f64,
    pub nominal_mm: f64,
    /// 示教框中心到最远站（mm）
    pub lever_mm: f64,
    pub spacing_mm: f64,
    /// 这个视图的站数
    pub stations: usize,
}

/// 一次 ±40 试定位：跑出了位姿（不论是否被采用），或没跑出结果的原因（测量报错、没有位姿）。
/// 没跑出结果不能当成“被拒”：±40 是必须过的检查，没跑就没证据。
pub type Probe = Result<PoseRecord, String>;

/// 一件独立正常样本在一个特征定位视图上的结果。
#[derive(Clone, Debug)]
pub struct FixtureSample {
    pub name: String,
    /// 按配方门限在原图上的定位；测量失败时为空
    pub pose: Option<PoseRecord>,
    /// 有基准站（ST_OK）的 d′（mm，未扣名义值）
    pub deviations_mm: Vec<f32>,
    /// 整图亮度 +40 / −40，按配方门限
    pub brighter: Probe,
    pub darker: Probe,
    /// ±3° 离线定位：原图、+40、−40
    pub rotation: Vec<Option<PoseRecord>>,
    pub error: Option<String>,
}

impl Default for FixtureSample {
    fn default() -> Self {
        Self { name: String::new(), pose: None, deviations_mm: Vec::new(), brighter: Err("没有运行".into()), darker: Err("没有运行".into()), rotation: Vec::new(), error: None }
    }
}

/// 示教两端参数的建议（定稿 §10 决定一）：只从验证统计来，用户确认后由后端写进配方。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndSuggestion {
    /// 两端 |Δ| 的 P99（mm）
    pub p99_mm: f64,
    /// maxEndShiftMm = P99 + 1 mm、示教余量 m = P99 + 1 个站距，向上取到 0.1 mm
    pub max_end_shift_mm: f32,
    pub end_margin_mm: f32,
    /// m 会让胶路剩不下 3 站可判时不能采用
    pub applicable: bool,
    pub note: String,
}

/// 开转角的证据（定稿 §10 决定二）：三个条件里前两个自动算，第三个（独立证据）只能人工确认。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationEvidence {
    pub enabled: bool,
    pub lever_mm: f64,
    /// ±3° 下原始验证帧 L·|θ| 的 P95（mm）
    pub lever_theta_p95_mm: Option<f64>,
    /// 公差/5
    pub threshold_mm: f64,
    pub exceeds: bool,
    /// ±3° 下全部验证帧（含亮度 ±40）都定位成功且看得出角度
    pub frames: usize,
    pub observable_frames: usize,
    pub all_observable: bool,
    /// 前两个条件都满足；还要人工给出独立证据才可以开转角
    pub auto_conditions: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureSampleRow {
    pub name: String,
    pub pose: Option<PoseRecord>,
    pub brighter: Option<PoseRecord>,
    pub darker: Option<PoseRecord>,
    pub error: Option<String>,
}

/// 一个特征定位视图在独立正常样本上的验证统计（`Validation.fixture`）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FixtureViewReport {
    pub k: usize,
    pub view: u8,
    pub shot_id: String,
    pub passed: bool,
    pub samples: usize,
    pub checks: Vec<CheckItem>,
    /// 全部正常样本、全部有基准站的 d′ 合并算的 σ 与 μ（mm，D2 口径）
    pub sigma_mm: Option<f64>,
    pub mean_mm: Option<f64>,
    pub nominal_mm: f64,
    pub tolerance_mm: f64,
    /// 位姿单轴偏移 / 搜索半径的最大值
    pub search_use: Option<f64>,
    pub suggestion: Option<EndSuggestion>,
    pub rotation: RotationEvidence,
    pub rows: Vec<FixtureSampleRow>,
}

/// 最近秩百分位：升序第 ⌈q·n⌉ 个。
fn nearest_rank(values: &[f64], q: f64) -> Option<f64> {
    if values.is_empty() { return None; }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = ((q * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    Some(sorted[rank - 1])
}

/// 向上取到 0.1 mm。
fn ceil_tenth(v: f64) -> f32 {
    ((v * 10.0 - 1e-6).ceil() / 10.0) as f32
}

fn accepted(p: &Option<PoseRecord>) -> Option<&PoseRecord> {
    p.as_ref().filter(|p| p.accepted)
}

/// 两次定位是否锁在同一处：模板中心差 ≤ 2 px、角度差 ≤ 0.3°。
fn same_lock(a: &PoseRecord, b: &PoseRecord) -> bool {
    let (Some(oa), Some(ob)) = (a.offset, b.offset) else { return false };
    let center = f64::from(oa[0] - ob[0]).hypot(f64::from(oa[1] - ob[1]));
    let angle = f64::from(a.angle.unwrap_or(0.0) - b.angle.unwrap_or(0.0)).abs();
    center <= LOCK_TOL_PX && angle <= LOCK_TOL_DEG
}

pub fn validation_report(ctx: &ViewContext, samples: &[FixtureSample]) -> FixtureViewReport {
    let f = &ctx.fixture;
    let rotation_on = f.angle_range_deg > 0.0;
    let mut checks = Vec::new();
    let names = |list: Vec<String>| { let shown: Vec<_> = list.iter().take(5).cloned().collect(); format!("{}{}", shown.join("、"), if list.len() > 5 { format!(" 等 {} 件", list.len()) } else { String::new() }) };

    // 每件都要定位成功
    let failed: Vec<String> = samples.iter().filter(|s| accepted(&s.pose).is_none()).map(|s| match (&s.pose, &s.error) {
        (_, Some(e)) => format!("{}（{e}）", s.name),
        (Some(p), None) => format!("{}（{}）", s.name, reason_text(p.reason.as_deref())),
        (None, None) => format!("{}（没有定位结果）", s.name),
    }).collect();
    checks.push(CheckItem::new(CheckId::LocateAll, Level::Block, !samples.is_empty() && failed.is_empty(), if samples.is_empty() {
        "没有独立正常样本".into()
    } else if failed.is_empty() {
        format!("{} 件独立正常样本全部定位成功", samples.len())
    } else {
        format!("{} 件定位失败：{}", failed.len(), names(failed.clone()))
    }));

    // σ、μ：全部正常样本、全部有基准站合并（D2）
    let pooled: Vec<f64> = samples.iter().filter(|s| accepted(&s.pose).is_some()).flat_map(|s| s.deviations_mm.iter().map(|&d| f64::from(d))).filter(|d| d.is_finite()).collect();
    let mean = (!pooled.is_empty()).then(|| pooled.iter().sum::<f64>() / pooled.len() as f64);
    let sigma = mean.filter(|_| pooled.len() >= 2).map(|m| (pooled.iter().map(|d| (d - m).powi(2)).sum::<f64>() / (pooled.len() - 1) as f64).sqrt());
    let sigma_limit = ctx.tolerance_mm / 5.0;
    checks.push(CheckItem::new(CheckId::Sigma, Level::Block, sigma.is_some_and(|s| s <= sigma_limit), match sigma {
        Some(s) if s <= sigma_limit => format!("d′ 的 σ = {s:.3} mm（≤ 公差/5 = {sigma_limit:.3} mm，{} 站）", pooled.len()),
        Some(s) => format!("d′ 的 σ = {s:.3} mm，超过公差/5 = {sigma_limit:.3} mm：定位或胶检测不够稳，判胶偏会误报"),
        None => "有基准的站不足 2 个，算不出 σ".into(),
    }));
    if let Some(m) = mean {
        let off = (m - ctx.nominal_mm).abs();
        let limit = ctx.tolerance_mm / 3.0;
        checks.push(CheckItem::new(CheckId::Nominal, Level::Warn, off <= limit, if off <= limit {
            format!("μ = {m:+.3} mm，与名义值 {:+.3} mm 相差 {off:.3} mm（≤ 公差/3）", ctx.nominal_mm)
        } else {
            format!("μ = {m:+.3} mm，与名义值 {:+.3} mm 相差 {off:.3} mm，超过公差/3 = {limit:.3} mm。不会自动校正：确认是夹具或示教偏了，还是要用量具确认件设名义值", ctx.nominal_mm)
        }));
    }

    // 搜索范围占用
    let radius = f64::from(f.search_radius_px);
    let search_use = samples.iter().filter_map(|s| accepted(&s.pose)?.offset).map(|o| f64::from(o[0].abs().max(o[1].abs())) / radius).fold(None, |a: Option<f64>, v| Some(a.map_or(v, |a| a.max(v))));
    if let Some(u) = search_use {
        checks.push(CheckItem::new(CheckId::SearchRange, Level::Warn, u <= SEARCH_WARN, if u <= SEARCH_WARN {
            format!("位姿最多用到搜索半径的 {:.0}%", u * 100.0)
        } else {
            format!("位姿用到搜索半径的 {:.0}%（超过 {:.0}%）：工件偏得再多一点就会贴边失败，考虑加大搜索半径或改善夹具", u * 100.0, SEARCH_WARN * 100.0)
        }));
    }

    // 亮度 ±40（必须过的检查，定稿 §9、§10）：错锁、没跑出结果、一次锁对都没有 → 阻断；部分被拒只告警；
    // 开了转角时看不出角度另算
    let (mut wrong, mut rejected, mut unobservable, mut missing, mut locked) = (Vec::new(), Vec::new(), Vec::new(), Vec::new(), 0usize);
    for s in samples {
        let Some(base) = accepted(&s.pose) else { continue };
        for (sign, probe) in [("+40", &s.brighter), ("−40", &s.darker)] {
            match probe {
                Err(e) => missing.push(format!("{} {sign}（{e}）", s.name)),
                Ok(p) if p.accepted && !same_lock(p, base) => wrong.push(format!("{} {sign}", s.name)),
                Ok(p) if p.accepted => locked += 1,
                Ok(p) if p.reason.as_deref() == Some("angle_unobservable") => unobservable.push(format!("{} {sign}", s.name)),
                Ok(p) => rejected.push(format!("{} {sign}（{}）", s.name, reason_text(p.reason.as_deref()))),
            }
        }
    }
    let probed = samples.iter().filter(|s| accepted(&s.pose).is_some()).count();
    let runs = 2 * probed;
    checks.push(CheckItem::new(CheckId::BrightnessLock, Level::Block, probed > 0 && wrong.is_empty() && missing.is_empty() && locked > 0, if probed == 0 {
        "没有可做亮度 ±40 试定位的样本".into()
    } else if !wrong.is_empty() {
        format!("亮度 ±{BRIGHTNESS_PROBE} 下错锁：{}。错锁会报成功、给出错的位姿，请换框", names(wrong.clone()))
    } else if !missing.is_empty() {
        format!("亮度 ±{BRIGHTNESS_PROBE} 试定位没有跑出结果：{}。这是必须过的检查，没跑就没有证据，排查后重新验证", names(missing.clone()))
    } else if locked == 0 {
        format!("亮度 ±{BRIGHTNESS_PROBE} 下 {runs} 次试定位全部没被采用，证明不了这个框在亮度变化下锁得对：请换框或改善照明")
    } else {
        format!("亮度 ±{BRIGHTNESS_PROBE} 下 {runs} 次试定位里 {locked} 次锁在原位姿、没有错锁（偏离原位姿 > {LOCK_TOL_PX} px 或 > {LOCK_TOL_DEG}° 算错锁）")
    }));
    if locked > 0 && (!rejected.is_empty() || (!rotation_on && !unobservable.is_empty())) {
        let all: Vec<String> = rejected.iter().chain(&unobservable).cloned().collect();
        checks.push(CheckItem::new(CheckId::BrightnessReject, Level::Warn, false,
            format!("亮度 ±{BRIGHTNESS_PROBE} 下没被采用：{}。现场亮度变化这么大时会判 ERR 92", names(all))));
    }
    if rotation_on {
        let unobservable_main: Vec<String> = samples.iter().filter(|s| s.pose.as_ref().is_some_and(|p| p.reason.as_deref() == Some("angle_unobservable"))).map(|s| s.name.clone()).collect();
        let all: Vec<String> = unobservable_main.into_iter().chain(unobservable).collect();
        checks.push(CheckItem::new(CheckId::Observable, Level::Block, all.is_empty(), if all.is_empty() {
            "开了转角：全部验证帧（含亮度 ±40）都看得出角度".into()
        } else {
            format!("开了转角，但这些帧看不出角度：{}（运行时会判定位失败）", names(all))
        }));
    }

    // 转角证据（决定二）
    let thetas: Vec<f64> = samples.iter().filter_map(|s| s.rotation.first()?.as_ref().filter(|p| p.accepted)?.angle)
        .map(|a| ctx.lever_mm * f64::from(a).to_radians().abs()).collect();
    let rotation_frames: Vec<&Option<PoseRecord>> = samples.iter().flat_map(|s| s.rotation.iter()).collect();
    let observable_frames = rotation_frames.iter().filter(|p| p.as_ref().is_some_and(|p| p.accepted)).count();
    let p95 = nearest_rank(&thetas, 0.95);
    let threshold = ctx.tolerance_mm / 5.0;
    let exceeds = p95.is_some_and(|v| v > threshold);
    let all_observable = !rotation_frames.is_empty() && observable_frames == rotation_frames.len();
    let rotation = RotationEvidence {
        enabled: rotation_on, lever_mm: ctx.lever_mm, lever_theta_p95_mm: p95, threshold_mm: threshold, exceeds,
        frames: rotation_frames.len(), observable_frames, all_observable, auto_conditions: exceeds && all_observable,
    };
    if !rotation_on && exceeds {
        checks.push(CheckItem::new(CheckId::Rotation, Level::Warn, false, format!(
            "±{ROTATION_PROBE_DEG}° 离线定位估出的转角在最远站造成 {:.3} mm（P95），超过公差/5 = {threshold:.3} mm：只找平移补不了。\
             请改夹具，或在有独立证据时到示教页高级设置里开转角", p95.unwrap_or_default())));
    }

    // 两端切向搬移 → 建议（决定一）
    let shifts: Vec<f64> = samples.iter().filter_map(|s| accepted(&s.pose)?.end_shift_mm).flat_map(|d| d.map(|v| f64::from(v).abs())).collect();
    let suggestion = nearest_rank(&shifts, 0.99).map(|p99| {
        let max_end_shift_mm = ceil_tenth(p99 + 1.0);
        let end_margin_mm = ceil_tenth(p99 + ctx.spacing_mm);
        let applicable = judged_stations(ctx.stations, ctx.spacing_mm, f64::from(end_margin_mm)) >= MIN_JUDGED_STATIONS;
        EndSuggestion { p99_mm: p99, max_end_shift_mm, end_margin_mm, applicable, note: if applicable {
            format!("两端 |Δ| 的 P99 = {p99:.2} mm（{} 个端点）：护栏取 P99 + 1 mm，示教余量取 P99 + 1 个站距；两端各长 m 的一段（起胶、收胶）以后永远不判", shifts.len())
        } else {
            format!("示教余量 {end_margin_mm:.1} mm 会让胶路剩不下 {MIN_JUDGED_STATIONS} 站可判，不能采用：请换更稳的定位框、改善夹具，或缩短搬移")
        } }
    });
    // 两端参数是否已按建议设置（决定一第 2、3 条）：没采用时护栏还是 G4 之前的临时上限，只告警（§13）
    let limit = samples.iter().find_map(|s| accepted(&s.pose)).map(|p| p.end_limit_mm);
    let end_item = match (&suggestion, f.max_end_shift_mm) {
        (_, None) => CheckItem::new(CheckId::EndParams, Level::Warn, false, format!(
            "两端参数还没采用验证建议：两端搬移上限仍是 G4 之前的临时默认（√2·(搜索半径 − 2)·mm/px{}），示教余量 m = {} mm；每端最多约 |Δ| + 1 个站距不判也不报 ERR",
            limit.map(|l| format!("，约 {l:.1} mm")).unwrap_or_default(), f.end_margin_mm)),
        (Some(s), Some(max)) if s.applicable && (max != s.max_end_shift_mm || f.end_margin_mm != s.end_margin_mm) => CheckItem::new(CheckId::EndParams, Level::Warn, false, format!(
            "两端参数与本次验证建议不同：现在两端搬移上限 {max} mm、示教余量 m {} mm，建议 {} mm、{} mm", f.end_margin_mm, s.max_end_shift_mm, s.end_margin_mm)),
        (_, Some(max)) => CheckItem::new(CheckId::EndParams, Level::Warn, true, format!("两端搬移上限 {max} mm、示教余量 m {} mm", f.end_margin_mm)),
    };
    checks.push(end_item);
    if samples.len() < FEW_SAMPLES {
        checks.push(CheckItem::new(CheckId::FewSamples, Level::Warn, false,
            format!("只有 {} 件独立正常样本：P99 就是最大值，σ 也不稳；上线前至少采 {FEW_SAMPLES} 件", samples.len())));
    }

    FixtureViewReport {
        k: ctx.k, view: ctx.view, shot_id: ctx.shot_id.clone(), passed: first_block(&checks).is_none(), samples: samples.len(), checks,
        sigma_mm: sigma, mean_mm: mean, nominal_mm: ctx.nominal_mm, tolerance_mm: ctx.tolerance_mm, search_use, suggestion, rotation,
        rows: samples.iter().map(|s| FixtureSampleRow { name: s.name.clone(), pose: s.pose.clone(), brighter: s.brighter.clone().ok(), darker: s.darker.clone().ok(), error: s.error.clone() }).collect(),
    }
}

#[cfg(test)]
#[path = "fixture_quality/tests.rs"]
mod tests;
