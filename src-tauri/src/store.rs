use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::burst_judge::{BurstJudgement, Flags, FrameMetrics, FrameState};
use crate::cycle::{FrameStatus, FrameView};
use crate::judge::{Judgement, PointState, Verdict};
use crate::offline::sample_set::SampleRef;
use crate::plan::PathSegment;
use crate::recipe::Recipe;

/// 一件工件的完整结果，入库一行；测量点按弧长顺序压成一个 BLOB，格式见 POINTS_FORMAT。
///
/// 段配方（`burst` 有值）按每相机每帧落库到 `part_frames`，`shots` / `table` / `frames` 为空，
/// `judgement` 是区域判定的摘要（verdict / plc_code / fault_code / reason 与 `burst.judgement` 相同，segments、gaps 为空）；
/// `frames_expected` = expected_total（待标定为 0）。旧拍照点配方 `burst` 为 None，行为不变。
pub struct PartRecord<'a> {
    pub mode: PartMode,
    pub ts: i64,
    pub sn: &'a str,
    pub recipe: Option<&'a Recipe>,
    pub judgement: &'a Judgement,
    pub drain_ms: Option<u64>,
    pub frames: &'a [FrameView],
    pub frames_expected: usize,
    pub frames_received: usize,
    pub triggers: u64,
    pub table: Option<&'a [PointState]>,
    pub software_version: &'a str,
    pub cycle_id: Option<&'a str>,
    pub bundle_id: Option<&'a str>,
    pub delivery: &'a PlcDelivery,
    pub shots: &'a [PartShot],
    pub source: RecordSource,
    pub sample: Option<&'a SampleRef>,
    /// 段配方的帧与区域记录；旧拍照点配方为 None
    pub burst: Option<&'a BurstRecord>,
}

/// 段配方一件的帧与区域记录（D-SB-29）：每台相机每一帧一条 `FrameRecord`，缺帧也有；区域结论与标记在 `judgement`。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BurstRecord {
    /// ΣN_exp；未标定为 None
    pub expected_total: Option<u32>,
    pub tolerance_total: u32,
    /// PLC partEnd 时的 deviceTriggers，只作参考；没有收到 partEnd 为 None
    pub device_triggers: Option<u32>,
    /// 各相机已接受的帧数，按计划相机顺序
    pub camera_frames: Vec<u32>,
    pub judgement: BurstJudgement,
    pub frames: Vec<FrameRecord>,
}

/// 一台相机一帧的记录（IPC 契约见 docs/decisions/segment-burst.md 5.2）。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameRecord {
    /// 计划相机下标
    pub camera: usize,
    /// 帧组序号，从 1 起
    pub seq: u64,
    pub status: FrameState,
    pub frame_counter: Option<u64>,
    pub trigger_counter: Option<u64>,
    /// `BurstJudgement.regions` 的下标；不在任何区域为 None
    pub region: Option<usize>,
    /// 缺帧时为 None
    pub metrics: Option<FrameMetrics>,
    /// 录制根内相对路径；未录制或录制队满丢弃为 None。入库后只增不换
    pub raw_file: Option<String>,
}

/// 段配方一帧的身份：(计划相机下标, 帧组序号)。原图引用按它关联（旧拍照点模型按 (k, view)）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameKey {
    pub camera: usize,
    pub seq: u64,
}

/// 历史详情里段配方的部分（IPC 契约见 docs/decisions/segment-burst.md 5.2）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartDetailBurst {
    /// 本件配方快照的段
    pub segments: Vec<PathSegment>,
    /// 本件配方快照的计划相机
    pub cameras: Vec<String>,
    pub expected_total: Option<u32>,
    pub tolerance_total: u32,
    pub device_triggers: Option<u32>,
    pub camera_frames: Vec<u32>,
    pub judgement: BurstJudgement,
    /// 按 (camera, seq) 排序
    pub frames: Vec<FrameRecord>,
}

/// 历史列表里段配方记录的帧数（只供 CSV 导出，不进 IPC：界面契约只有 `PartSummary.flags`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BurstSummary {
    pub expected_total: Option<u32>,
    pub tolerance_total: u32,
    pub device_triggers: Option<u32>,
    pub camera_frames: Vec<u32>,
}

/// 件的检测模式：维护模式下 PLC 按未检放行，采集件只录图不判定，试判件的结论不算产线合格率。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PartMode {
    #[default]
    Production,
    MaintenanceCollect,
    MaintenanceShadow,
}

/// 记录来源：按写库时的站点模式与离线来源决定。真机件与离线件分开统计、分开保存原图。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordSource {
    #[default]
    Real,
    OfflineSynthetic,
    OfflineSample,
    OfflineCandidate,
}

impl RecordSource {
    pub fn offline(self) -> bool {
        self != Self::Real
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Real => "真机",
            Self::OfflineSynthetic => "离线·合成图",
            Self::OfflineSample => "离线·样本集",
            Self::OfflineCandidate => "候选离线调试",
        }
    }
}

/// 历史查询范围：真机件或离线件，两者不混在一起统计。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HistoryScope {
    #[default]
    Real,
    Offline,
    Debug,
}

impl HistoryScope {
    pub fn of(source: RecordSource) -> Self {
        if source == RecordSource::OfflineCandidate { Self::Debug } else if source.offline() { Self::Offline } else { Self::Real }
    }

    fn clause(self, prefix: &str) -> String {
        match self {
            Self::Real => format!("{prefix}source = 'real'"),
            Self::Debug => format!("{prefix}source = 'offlineCandidate'"),
            Self::Offline => format!("{prefix}source IN ('offlineSynthetic','offlineSample')"),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlcDeliveryState {
    #[default]
    NotRequired,
    Pending,
    Submitted,
    Acknowledged,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlcDelivery {
    pub state: PlcDeliveryState,
    pub updated_at: i64,
    pub message: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingState {
    #[default]
    Pending,
    Off,
    NotRetained,
    Complete,
    Incomplete,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingEvidence {
    pub state: RecordingState,
    pub available: bool,
    pub directory: Option<String>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShotRawFile {
    pub view: u8,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartShot {
    pub k: usize,
    pub shot_id: String,
    pub camera: String,
    pub view: u8,
    pub session: Option<u64>,
    pub ordinal: Option<u64>,
    pub frame_counter: Option<u64>,
    pub trigger_counter: Option<u64>,
    pub status: FrameStatus,
    pub error: Option<String>,
    pub score: Option<f32>,
    pub ms: Option<u32>,
    pub raw_files: Vec<ShotRawFile>,
}

/// 7：帧状态去掉 locateFailed、帧加 referenceFailed、测量点 BLOB 格式 3。
/// 8：帧的各视图结果加特征定位位姿（viewResults[].pose）。
/// 9：段配方按每相机每帧落库（表 part_frames；parts 加 expected_total、tolerance_total、device_triggers、camera_frames、flags、burst_judgement）。
/// 旧库按版本不符备份后重建（D-0）。
pub const DB_VERSION: i64 = 9;

/// 测量点 BLOB 格式版本：每点 f32 位置 + f32 胶宽 + u8 状态（9 字节），没有的值写 NaN，状态说了算。
/// 3 起新增状态 4 无基准、5 超出检测范围；只读当前格式，旧格式的库已在打开时整库备份重建。
const POINTS_FORMAT: i64 = 3;

const ST_MEASURED: u8 = 0;
const ST_GAP: u8 = 1;
const ST_INVALID: u8 = 2;
const ST_PENDING: u8 = 3;
const ST_NO_REF: u8 = 4;
const ST_OUT_OF_RANGE: u8 = 5;

fn encode(table: &[PointState]) -> Vec<u8> {
    let mut out = Vec::with_capacity(table.len() * 9);
    for p in table {
        let (d, w, st) = match *p {
            PointState::Measured { d: Some(d), w } => (d, w, ST_MEASURED),
            PointState::Measured { d: None, w } => (f32::NAN, w, ST_NO_REF),
            PointState::OutOfRange { w } => (f32::NAN, w.unwrap_or(f32::NAN), ST_OUT_OF_RANGE),
            PointState::Gap => (0.0, f32::NAN, ST_GAP),
            PointState::Invalid => (0.0, f32::NAN, ST_INVALID),
            PointState::Pending => (0.0, f32::NAN, ST_PENDING),
        };
        out.extend_from_slice(&d.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.push(st);
    }
    out
}

/// 每点的 (位置, 胶宽, 状态)。
fn raw_points(format: i64, blob: &[u8]) -> Result<Vec<(f32, f32, u8)>, String> {
    if format != POINTS_FORMAT {
        return Err(format!("测量点数据格式 {format} 不是当前格式 {POINTS_FORMAT}，不读取"));
    }
    if blob.len() % 9 != 0 { return Err("测量点数据长度不是整点".into()); }
    let f = |c: &[u8]| f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
    Ok(blob.chunks_exact(9).map(|c| (f(c), f(&c[4..]), c[8])).collect())
}

fn point_state(d: f32, w: f32, st: u8) -> Result<PointState, String> {
    Ok(match st {
        ST_MEASURED => PointState::Measured { d: Some(d), w },
        ST_NO_REF => PointState::Measured { d: None, w },
        ST_OUT_OF_RANGE => PointState::OutOfRange { w: w.is_finite().then_some(w) },
        ST_GAP => PointState::Gap,
        ST_INVALID => PointState::Invalid,
        ST_PENDING => PointState::Pending,
        _ => return Err(format!("测量点状态 {st} 未知")),
    })
}

pub fn decode(format: i64, blob: &[u8]) -> Result<Vec<PointState>, String> {
    raw_points(format, blob)?.into_iter().map(|(d, w, st)| point_state(d, w, st)).collect()
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HistoryQuery {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub verdicts: Vec<Verdict>,
    pub sn: Option<String>,
    pub recipe_id: Option<String>,
    pub scope: HistoryScope,
    /// 为空不按模式筛选
    pub modes: Vec<PartMode>,
    pub offset: u32,
    pub limit: u32,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartSummary {
    pub id: i64,
    pub ts: i64,
    pub sn: String,
    pub recipe_id: Option<String>,
    pub recipe_version: Option<u32>,
    pub recipe_revision: Option<String>,
    pub trigger_mode: Option<String>,
    pub verdict: Verdict,
    pub plc_code: u16,
    pub fault_code: u16,
    pub reason: String,
    pub drain_ms: Option<u64>,
    pub frames_expected: usize,
    pub frames_received: usize,
    pub retest_of: Option<i64>,
    pub cycle_id: Option<String>,
    pub bundle_id: Option<String>,
    pub delivery: PlcDelivery,
    pub source: RecordSource,
    /// 离线样本集运行的样本件引用，用于追溯和读取对应原图。
    pub sample: Option<SampleRef>,
    /// 段配方记录的整件标记（疑似提前终止等单独显示）；旧记录为 None
    pub flags: Option<Flags>,
    /// 段配方记录的帧数；旧记录为 None。只供 CSV 导出
    #[serde(skip)]
    pub burst: Option<BurstSummary>,
    pub mode: PartMode,
}

/// 维护采集的进度：每个配方已录件数与完整可用件数（录制完整、各计划相机帧数一致且非零），供判断是否够示教与标定。
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionProgress {
    pub recipe_id: String,
    pub recorded: u32,
    pub usable: u32,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct VerdictCounts {
    pub ok: u64,
    pub excursion: u64,
    pub ng: u64,
    pub err: u64,
}

impl VerdictCounts {
    pub fn add(&mut self, v: Verdict, n: u64) {
        match v {
            Verdict::Ok => self.ok += n,
            Verdict::OkWithExcursion => self.excursion += n,
            Verdict::ErrInspect => self.err += n,
            _ => self.ng += n,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryPage {
    pub total: u64,
    pub counts: VerdictCounts,
    pub items: Vec<PartSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartPoints {
    /// 位置；只有有基准的有胶站（状态 0）有值，其余为 null
    pub d: Vec<Option<f32>>,
    /// 胶宽；没测胶宽的点为 null
    pub w: Vec<Option<f32>>,
    /// 0 有胶 · 1 断胶 · 2 测不了 · 3 未测 · 4 有胶无基准 · 5 超出检测范围
    pub st: Vec<u8>,
    #[serde(skip)]
    table: Vec<PointState>,
}

impl PartPoints {
    /// 判定用的点状态表（按存储状态解码，与 `measurements` 重判同一口径）。
    pub fn table(&self) -> &[PointState] {
        &self.table
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartDetail {
    pub summary: PartSummary,
    pub judgement: Judgement,
    pub frames: Vec<FrameView>,
    pub triggers: u64,
    pub software_version: String,
    pub points: Option<PartPoints>,
    pub retests: Vec<i64>,
    pub shots: Vec<PartShot>,
    pub recording: RecordingEvidence,
    /// 段配方记录；旧记录为 None（有值时 frames、shots 为空，points 为 None）
    pub burst: Option<PartDetailBurst>,
}

/// 重判用：一件工件的判定输入。
pub struct StoredMeasurement {
    pub id: i64,
    pub ts: i64,
    pub sn: String,
    pub recipe_id: Option<String>,
    pub recipe_revision: Option<String>,
    pub verdict: Verdict,
    pub table: Vec<PointState>,
    /// 段配方记录当时的区域判定（区域、丢帧告警）；旧记录为 None。逐帧指标按需用 `Store::burst_frames` 读
    pub burst: Option<BurstJudgement>,
}

pub struct Store {
    conn: Mutex<Connection>,
    backup_path: Option<PathBuf>,
}

fn verdict_str(v: Verdict) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

fn parse_verdict(s: &str) -> Verdict {
    serde_json::from_value(serde_json::Value::String(s.into())).unwrap_or(Verdict::ErrInspect)
}

const SUMMARY_COLS: &str = "id, ts, sn, recipe_id, recipe_version, recipe_revision, trigger_mode, verdict, plc_code, fault_code, \
                            reason, drain_ms, frames_expected, frames_received, retest_of, cycle_id, bundle_id, \
                            delivery_state, delivery_updated_at, delivery_message, sample, source, \
                            flags, expected_total, tolerance_total, device_triggers, camera_frames, mode";
/// SUMMARY_COLS 的列数；detail 在其后追加的列从这里起算。
const SUMMARY_LEN: usize = 28;

fn summary(row: &rusqlite::Row) -> rusqlite::Result<PartSummary> {
    let flags: Option<Flags> = row.get::<_, Option<String>>(22)?.map(|text| serde_json::from_str(&text)
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(22, rusqlite::types::Type::Text, Box::new(e)))).transpose()?;
    let burst = match flags {
        Some(_) => Some(BurstSummary {
            expected_total: row.get(23)?,
            tolerance_total: row.get(24)?,
            device_triggers: row.get(25)?,
            camera_frames: json_column(row, 26)?,
        }),
        None => None,
    };
    Ok(PartSummary {
        id: row.get(0)?,
        ts: row.get(1)?,
        sn: row.get(2)?,
        recipe_id: row.get(3)?,
        recipe_version: row.get(4)?,
        recipe_revision: row.get(5)?,
        trigger_mode: row.get(6)?,
        verdict: parse_verdict(&row.get::<_, String>(7)?),
        plc_code: row.get(8)?,
        fault_code: row.get(9)?,
        reason: row.get(10)?,
        drain_ms: row.get::<_, Option<i64>>(11)?.map(|v| v as u64),
        frames_expected: row.get::<_, i64>(12)? as usize,
        frames_received: row.get::<_, i64>(13)? as usize,
        retest_of: row.get(14)?,
        cycle_id: row.get(15)?,
        bundle_id: row.get(16)?,
        delivery: delivery_row(row, 17)?,
        sample: row.get::<_, Option<String>>(20)?.map(|text| serde_json::from_str(&text)
            .map_err(|e| rusqlite::Error::FromSqlConversionFailure(20, rusqlite::types::Type::Text, Box::new(e)))).transpose()?,
        source: enum_column(row, 21)?,
        flags,
        burst,
        mode: enum_column(row, 27)?,
    })
}

fn db_err(e: rusqlite::Error) -> String {
    format!("检测记录数据库错误：{e}")
}

fn enum_name(value: impl Serialize) -> String {
    serde_json::to_value(value).unwrap().as_str().unwrap().to_string()
}

fn json_column<T: serde::de::DeserializeOwned>(row: &rusqlite::Row, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_str(&text).map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e)))
}

fn recording_row(row: &rusqlite::Row, index: usize) -> rusqlite::Result<RecordingEvidence> {
    Ok(RecordingEvidence {
        state: enum_column(row, index)?,
        available: row.get(index + 1)?,
        directory: row.get(index + 2)?,
        errors: json_column(row, index + 3)?,
    })
}

fn enum_column<T: serde::de::DeserializeOwned>(row: &rusqlite::Row, index: usize) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    serde_json::from_value(serde_json::Value::String(text))
        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e)))
}

fn count_column(row: &rusqlite::Row, index: usize) -> rusqlite::Result<Option<u64>> {
    row.get::<_, Option<String>>(index)?
        .map(|s| s.parse::<u64>().map_err(|e| rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))))
        .transpose()
}

fn delivery_row(row: &rusqlite::Row, first: usize) -> rusqlite::Result<PlcDelivery> {
    Ok(PlcDelivery { state: enum_column(row, first)?, updated_at: row.get(first + 1)?, message: row.get(first + 2)? })
}

fn measurement_layout(recipe: &Recipe) -> serde_json::Value {
    let shots: Vec<_> =
        recipe.shots.iter().map(|s| {
            // fixture 是测量定义（定位模板与 maxEndShiftMm 都会改变站状态），改了要原图重测；名义值与 gaugeRef 是判定参数，不在这里。
            // fixture 里含模板来源 source：模板身份 = (来源冻结图, 框, 外圈)，不用内容哈希（D-0）
            let views: Vec<_> = s.views.iter().map(|v| serde_json::json!([v.view, v.enabled, v.path, v.mm_per_px, v.calib, v.detect, v.fixture])).collect();
            serde_json::json!([s.id, s.pose_id, s.camera, s.view, s.skip, s.path, s.mm_per_px, s.calib_ref(), s.detect, views, s.fixture])
        }).collect();
    let segments: Vec<_> = recipe.segments.iter().map(|s| (s.shot, s.view, s.first, s.count)).collect();
    // 模板裁法（vision::fixture::template_pgm）也属于测量定义：裁法一改，这里必须跟着改，否则 (来源, 框, 外圈) 不变、模板像素却变了
    let mut layout = serde_json::json!({"unit":"mm", "schema":recipe.schema_version, "spacing":recipe.spacing,
        "shots":shots, "segments":segments, "points":recipe.points, "detect":recipe.detect,
        "fixtureTemplate": {"pad": crate::recipe::FIXTURE_TEMPLATE_PAD_PX}});
    if recipe.is_burst() {
        // 段配方：各相机的走廊、当量、标定、检测参数决定每帧指标（计划身份另见 burst_plan_identity）；
        // 限值、容差、x_ref、检测区间不在这里，改了可以直接用已存指标重判。旧拍照点配方不加这一项，布局与原来相同。
        layout["inspect"] = serde_json::to_value(&recipe.inspect).unwrap_or_default();
    }
    layout
}

/// 段配方的计划身份（规格 §1：段序列 / 类型 / N_exp、相机、ΣN_exp、产品代码）；旧拍照点配方为 None，不参与比较。
fn burst_plan_identity(recipe: &Recipe) -> Option<crate::plan::PlanIdentity> {
    recipe.is_burst().then(|| crate::plan::plan_identity(recipe.product_code, &recipe.path_segments, &recipe.plan_cameras(), recipe.expected_total()))
}

/// 旧测量值（含 d′）能否在候选上复用。d′ 依赖定位模板的像素，模板身份 (来源冻结图, 框, 外圈) 在测量布局里，布局相同即同一模板。
/// `Recipe` 只能经 `RecipeDoc::build` 构造（反序列化也是），配方校验要求启用的 fixture 来源合法，不存在“来源未知”的配方。
/// 段配方另要求计划身份相同（计划身份变了，区域与预期帧数的口径就变了，需要原图复测）。
pub fn same_measurement_layout(original: &Recipe, candidate: &Recipe) -> bool {
    measurement_layout(original) == measurement_layout(candidate) && burst_plan_identity(original) == burst_plan_identity(candidate)
}

fn validate_raw_files(files: &[ShotRawFile]) -> Result<(), String> {
    let mut views = std::collections::HashSet::new();
    for file in files {
        if !matches!((file.width, file.height), (None, None) | (Some(1..), Some(1..))) {
            return Err("原图尺寸必须同时提供有效宽高".into());
        }
        if !(1..=3).contains(&file.view) || !views.insert(file.view) {
            return Err("原图元数据的视角越界或重复".into());
        }
        validate_raw_path(&file.file)?;
    }
    Ok(())
}

fn validate_raw_path(file: &str) -> Result<(), String> {
    if file.is_empty()
        || file.contains('\\')
        || file.contains(':')
        || file.contains('\0')
        || Path::new(file).components().any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("原图文件必须是录制根目录内的相对路径，使用 / 分隔".into());
    }
    Ok(())
}

fn finite_metrics(m: &FrameMetrics) -> bool {
    let finite = |value: Option<f64>| value.is_none_or(f64::is_finite);
    finite(m.width_min) && finite(m.width_max) && finite(m.width_mean) && finite(m.x_mm)
        && m.spacing_mm.is_finite() && m.gap_max_mm.is_finite() && m.present_ratio.is_finite()
        && m.widths.iter().flatten().all(|w| w.is_finite())
}

/// 段配方记录的约束（D-SB-29）：帧数不要求等于计划（规格 §2），但每帧身份 (camera, seq) 唯一、相机属于计划相机（§3）；
/// 区域下标、指标身份与原配方一致；摘要结论与区域判定相同。
fn validate_burst(r: &PartRecord, recipe: &Recipe, burst: &BurstRecord) -> Result<(), String> {
    if !recipe.is_burst() {
        return Err("旧拍照点配方的记录不能带段配方帧记录".into());
    }
    if !r.shots.is_empty() || r.table.is_some() || !r.frames.is_empty() {
        return Err("段配方记录按帧落库，不能带拍照点、测量点或旧帧信息".into());
    }
    let j = &burst.judgement;
    if (r.judgement.verdict, r.judgement.plc_code, r.judgement.fault_code, r.judgement.reason.as_str()) != (j.verdict, j.plc_code, j.fault_code, j.reason.as_str()) {
        return Err("段配方记录的结论摘要与区域判定不一致".into());
    }
    if burst.expected_total != recipe.expected_total() || burst.tolerance_total != recipe.tolerance_total()
        || r.frames_expected != burst.expected_total.unwrap_or(0) as usize {
        return Err("段配方记录的预期帧组数或容差与原配方不一致".into());
    }
    let cameras = recipe.plan_cameras().len();
    if burst.camera_frames.len() != cameras {
        return Err(format!("段配方记录的各相机帧数应有 {cameras} 项"));
    }
    let mut seen = std::collections::HashSet::new();
    for frame in &burst.frames {
        let (camera, seq) = (frame.camera, frame.seq);
        if camera >= cameras {
            return Err(format!("帧记录的相机下标 {camera} 不在计划相机中"));
        }
        if seq == 0 || i64::try_from(seq).is_err() {
            return Err(format!("相机 {camera} 的帧组序号 {seq} 无效"));
        }
        if !seen.insert((camera, seq)) {
            return Err(format!("相机 {camera} 第 {seq} 帧重复记录"));
        }
        if frame.region.is_some_and(|region| region >= j.regions.len()) {
            return Err(format!("相机 {camera} 第 {seq} 帧引用的区域不存在"));
        }
        if let Some(metrics) = &frame.metrics {
            if (metrics.camera, metrics.seq, metrics.state) != (camera, seq, frame.status) || !finite_metrics(metrics) {
                return Err(format!("相机 {camera} 第 {seq} 帧的指标与帧身份不一致或不是有限数"));
            }
        }
        if let Some(file) = &frame.raw_file {
            validate_raw_path(file)?;
        }
    }
    Ok(())
}

/// 已入库的段配方记录与一份待入库记录是否同一检测内容；原图引用是事后补的证据，不参与比较。
fn same_burst(actual: Option<&PartDetailBurst>, expected: Option<&BurstRecord>) -> Result<bool, String> {
    let (actual, expected) = match (actual, expected) {
        (None, None) => return Ok(true),
        (Some(actual), Some(expected)) => (actual, expected),
        _ => return Ok(false),
    };
    let content = |frames: &[FrameRecord]| -> Result<serde_json::Value, String> {
        let mut frames: Vec<FrameRecord> = frames.iter().cloned().map(|frame| FrameRecord { raw_file: None, ..frame }).collect();
        frames.sort_by_key(|frame| (frame.camera, frame.seq));
        serde_json::to_value(frames).map_err(|error| error.to_string())
    };
    let judgement = |judgement: &BurstJudgement| serde_json::to_value(judgement).map_err(|error| error.to_string());
    Ok((actual.expected_total, actual.tolerance_total, actual.device_triggers, &actual.camera_frames)
        == (expected.expected_total, expected.tolerance_total, expected.device_triggers, &expected.camera_frames)
        && judgement(&actual.judgement)? == judgement(&expected.judgement)?
        && content(&actual.frames)? == content(&expected.frames)?)
}

fn frame_rows(conn: &Connection, id: i64) -> Result<Vec<FrameRecord>, String> {
    let mut stmt = conn
        .prepare("SELECT camera,seq,status,frame_counter,trigger_counter,region,metrics,raw_file FROM part_frames WHERE part_id=?1 ORDER BY camera, seq")
        .map_err(db_err)?;
    let rows = stmt
        .query_map([id], |row| {
            Ok(FrameRecord {
                camera: row.get::<_, i64>(0)? as usize,
                seq: row.get::<_, i64>(1)? as u64,
                status: enum_column(row, 2)?,
                frame_counter: count_column(row, 3)?,
                trigger_counter: count_column(row, 4)?,
                region: row.get::<_, Option<i64>>(5)?.map(|n| n as usize),
                metrics: row.get::<_, Option<String>>(6)?.map(|text| serde_json::from_str(&text)
                    .map_err(|e| rusqlite::Error::FromSqlConversionFailure(6, rusqlite::types::Type::Text, Box::new(e)))).transpose()?,
                raw_file: row.get(7)?,
            })
        })
        .map_err(db_err)?;
    rows.collect::<Result<_, _>>().map_err(db_err)
}

fn snapshot_in(conn: &Connection, revision_id: &str) -> Result<Option<Recipe>, String> {
    let snapshot: Option<(String, u32, String)> = conn.query_row(
        "SELECT recipe_id,version,json FROM recipe_snapshots WHERE revision_id = ?1", [revision_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(db_err)?;
    snapshot.map(|(id, version, json)| {
        let recipe = Recipe::from_snapshot(serde_json::from_str(&json).map_err(|e| e.to_string())?)?;
        if recipe.revision_id != revision_id || recipe.id != id || recipe.version != version {
            return Err("原配方快照身份或版本损坏".into());
        }
        Ok(recipe)
    })
    .transpose()
}

fn backup_old_database(conn: Connection, path: &Path, version: i64) -> Result<PathBuf, String> {
    conn.pragma_update(None, "locking_mode", "EXCLUSIVE").map_err(db_err)?;
    conn.execute_batch("BEGIN EXCLUSIVE; COMMIT;").map_err(db_err)?;
    let (busy, log, done): (i64, i64, i64) =
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).map_err(db_err)?;
    if busy != 0 || (log >= 0 && done < log) {
        return Err("旧记录库仍在使用，WAL 未完整合并；保留原库，关闭其他实例后重试".into());
    }
    let mode: String = conn.query_row("PRAGMA journal_mode = DELETE", [], |r| r.get(0)).map_err(db_err)?;
    if mode != "delete" {
        return Err("旧记录库无法退出 WAL 模式，保留原库".into());
    }
    let token: String = conn.query_row("SELECT lower(hex(randomblob(8)))", [], |r| r.get(0)).map_err(db_err)?;
    conn.close().map_err(|(_, e)| db_err(e))?;
    for suffix in ["-wal", "-shm"] {
        let mut name = path.as_os_str().to_os_string();
        name.push(suffix);
        if Path::new(&name).exists() {
            return Err("旧记录库仍有 WAL/SHM 文件，保留原库以防丢失数据".into());
        }
    }
    let name = path.file_name().ok_or("记录库路径没有文件名")?.to_string_lossy();
    let backup = path.with_file_name(format!("{name}.v{version}.{}.{token}.bak", ly_plc::now_ms()));
    std::fs::rename(path, &backup).map_err(|e| format!("备份旧记录库失败，原库未重建：{e}"))?;
    Ok(backup)
}

fn fail_interrupted_recordings(conn: &mut Connection) -> Result<usize, String> {
    const REASON: &str = "检测服务在录制完成前退出，原图完整性未确认";
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
    let pending = {
        let mut stmt = tx.prepare("SELECT id,recording_errors FROM parts WHERE recording_state='pending' ORDER BY id").map_err(db_err)?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, i64>(0)?, json_column::<Vec<String>>(row, 1)?))).map_err(db_err)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?
    };
    let mut interrupted = 0;
    for (id, mut errors) in pending {
        if !errors.iter().any(|error| error == REASON) {
            errors.push(REASON.into());
        }
        interrupted += tx.execute(
            "UPDATE parts SET recording_state='failed',recording_available=0,recording_errors=?2 WHERE id=?1 AND recording_state='pending'",
            params![id, serde_json::to_string(&errors).map_err(|e| e.to_string())?],
        ).map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(interrupted)
}

fn extend_candidate_source(conn: &Connection, path: &Path) -> Result<(), String> {
    let sql: Option<String> = conn.query_row("SELECT sql FROM sqlite_master WHERE type='table' AND name='parts'", [], |r| r.get(0)).optional().map_err(db_err)?;
    let Some(sql) = sql else { return Ok(()); };
    const OLD: &str = "CHECK(source IN ('real','offlineSynthetic','offlineSample'))";
    if !sql.contains(OLD) { return Ok(()); }
    let indexes = conn.prepare("SELECT sql FROM sqlite_master WHERE type='index' AND tbl_name='parts' AND sql IS NOT NULL").map_err(db_err)?
        .query_map([], |r| r.get::<_, String>(0)).map_err(db_err)?.collect::<Result<Vec<_>, _>>().map_err(db_err)?;
    let backup = path.with_extension(format!("before-candidate-{}.sqlite3", ly_plc::now_ms()));
    conn.execute("VACUUM INTO ?1", [backup.to_string_lossy().as_ref()]).map_err(|e| format!("调试记录格式升级前备份失败：{e}"))?;
    let definition = sql[sql.find('(').ok_or("记录表定义无效")?..].replace(OLD, "CHECK(source IN ('real','offlineSynthetic','offlineSample','offlineCandidate'))");
    conn.execute_batch("PRAGMA foreign_keys=OFF; BEGIN IMMEDIATE;").map_err(db_err)?;
    let migrated = conn.execute_batch(&format!("CREATE TABLE parts_candidate_upgrade {definition}; INSERT INTO parts_candidate_upgrade SELECT * FROM parts; DROP TABLE parts; ALTER TABLE parts_candidate_upgrade RENAME TO parts;"))
        .and_then(|_| {
            for index in &indexes { conn.execute_batch(index)?; }
            let violations: i64 = conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?;
            if violations != 0 { return Err(rusqlite::Error::InvalidQuery); }
            conn.execute_batch("COMMIT;")
        });
    if migrated.is_err() { let _ = conn.execute_batch("ROLLBACK;"); }
    conn.execute_batch("PRAGMA foreign_keys=ON;").map_err(db_err)?;
    migrated.map_err(|e| format!("调试记录格式升级失败，旧记录保持原样，备份 {}：{e}", backup.display()))
}

/// 记录库 9 版原地加 `mode` 列（维护模式），旧记录都是正式检测件，不重建、不丢记录。
fn add_part_mode(conn: &Connection) -> Result<(), String> {
    let sql: Option<String> = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type='table' AND name='parts'",
            [],
            |r| r.get(0),
        )
        .optional()
        .map_err(db_err)?;
    if sql.is_none_or(|sql| sql.contains(" mode TEXT")) {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE parts ADD COLUMN mode TEXT NOT NULL DEFAULT 'production' CHECK(mode IN ('production','maintenanceCollect','maintenanceShadow'))")
        .map_err(|e| format!("记录库加维护模式列失败：{e}"))
}

impl Store {
    #[cfg(test)]
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_inner(path, false)
    }

    pub fn open_deferred_recovery(path: &Path) -> Result<Self, String> {
        Self::open_inner(path, true)
    }

    fn open_inner(path: &Path, defer_recovery: bool) -> Result<Self, String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let existing = path.metadata().map(|m| m.len() > 0).unwrap_or(false);
        let mut conn = Connection::open(path).map_err(db_err)?;
        conn.busy_timeout(Duration::from_secs(2)).map_err(db_err)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0)).map_err(db_err)?;
        let backup_path = if existing && version != DB_VERSION {
            let backup = backup_old_database(conn, path, version)?;
            conn = Connection::open(path).map_err(|e| format!("旧记录库已备份到 {}；新库打开失败：{e}", backup.display()))?;
            conn.busy_timeout(Duration::from_secs(2)).map_err(db_err)?;
            Some(backup)
        } else {
            None
        };
        extend_candidate_source(&conn, path)?;
        add_part_mode(&conn)?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS cycle_ids (
                 id TEXT PRIMARY KEY NOT NULL DEFAULT (lower(hex(randomblob(16)))),
                 created_at INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS workspace_instances (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 recipe_id TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS parts (
                 id INTEGER PRIMARY KEY,
                 ts INTEGER NOT NULL,
                 sn TEXT NOT NULL,
                 recipe_id TEXT,
                 recipe_version INTEGER,
                 recipe_revision TEXT,
                 trigger_mode TEXT,
                 verdict TEXT NOT NULL,
                 plc_code INTEGER NOT NULL,
                 fault_code INTEGER NOT NULL,
                 reason TEXT NOT NULL,
                 drain_ms INTEGER,
                 frames_expected INTEGER NOT NULL,
                 frames_received INTEGER NOT NULL,
                 triggers INTEGER NOT NULL,
                 retest_of INTEGER,
                 software_version TEXT NOT NULL,
                 judgement TEXT NOT NULL,
                 frames TEXT NOT NULL,
                 cycle_id TEXT,
                 bundle_id TEXT,
                 delivery_state TEXT NOT NULL,
                 delivery_updated_at INTEGER NOT NULL,
                 delivery_message TEXT,
                 recording_state TEXT NOT NULL DEFAULT 'pending',
                 recording_available INTEGER NOT NULL DEFAULT 0,
                 recording_directory TEXT,
                 recording_errors TEXT NOT NULL DEFAULT '[]',
                 sample TEXT,
                 source TEXT NOT NULL CHECK(source IN ('real','offlineSynthetic','offlineSample','offlineCandidate')),
                 expected_total INTEGER,
                 tolerance_total INTEGER,
                 device_triggers INTEGER,
                 camera_frames TEXT,
                 flags TEXT,
                 burst_judgement TEXT,
                 mode TEXT NOT NULL DEFAULT 'production' CHECK(mode IN ('production','maintenanceCollect','maintenanceShadow'))
             );
             CREATE INDEX IF NOT EXISTS parts_ts ON parts(ts);
             CREATE INDEX IF NOT EXISTS parts_source_ts ON parts(source, ts);
             CREATE INDEX IF NOT EXISTS parts_sn ON parts(sn);
             CREATE INDEX IF NOT EXISTS parts_recording_directory ON parts(recording_directory);
             CREATE UNIQUE INDEX IF NOT EXISTS parts_cycle ON parts(cycle_id) WHERE cycle_id IS NOT NULL;
             CREATE INDEX IF NOT EXISTS parts_unresolved_delivery ON parts(cycle_id)
                 WHERE cycle_id IS NOT NULL AND delivery_state IN ('pending','submitted','failed');
             CREATE TABLE IF NOT EXISTS part_points (
                 part_id INTEGER PRIMARY KEY REFERENCES parts(id) ON DELETE CASCADE,
                 format INTEGER NOT NULL,
                 data BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS recipe_snapshots (
                 revision_id TEXT PRIMARY KEY,
                 recipe_id TEXT NOT NULL,
                 version INTEGER NOT NULL,
                 json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS part_shots (
                 part_id INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
                 k INTEGER NOT NULL,
                 shot_id TEXT NOT NULL,
                 camera TEXT NOT NULL,
                 view INTEGER NOT NULL CHECK(view BETWEEN 1 AND 3),
                 session TEXT,
                 ordinal TEXT,
                 frame_counter TEXT,
                 trigger_counter TEXT,
                 status TEXT NOT NULL,
                 error TEXT,
                 score REAL,
                 ms INTEGER,
                 raw_files TEXT NOT NULL,
                 PRIMARY KEY (part_id, k),
                 UNIQUE (part_id, shot_id)
             );
             CREATE TABLE IF NOT EXISTS part_frames (
                 part_id INTEGER NOT NULL REFERENCES parts(id) ON DELETE CASCADE,
                 camera INTEGER NOT NULL CHECK(camera >= 0),
                 seq INTEGER NOT NULL CHECK(seq >= 1),
                 status TEXT NOT NULL,
                 frame_counter TEXT,
                 trigger_counter TEXT,
                 region INTEGER,
                 metrics TEXT,
                 raw_file TEXT,
                 PRIMARY KEY (part_id, camera, seq)
             );
             PRAGMA user_version = 9;
             COMMIT;",
        )
        .map_err(db_err)?;
        if !defer_recovery { fail_interrupted_recordings(&mut conn)?; }
        Ok(Self { conn: Mutex::new(conn), backup_path })
    }

    pub fn finish_interrupted_recordings(&self) -> Result<usize, String> {
        fail_interrupted_recordings(&mut self.conn.lock().unwrap())
    }

    pub fn backup_path(&self) -> Option<&Path> {
        self.backup_path.as_deref()
    }

    pub fn recipe_version_floors(&self) -> Result<std::collections::BTreeMap<String, u32>, String> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT recipe_id,MAX(version) FROM (
            SELECT recipe_id,recipe_version AS version FROM parts WHERE recipe_id IS NOT NULL AND recipe_version IS NOT NULL AND source != 'offlineCandidate'
            UNION ALL SELECT recipe_id,version FROM recipe_snapshots s WHERE revision_id = recipe_id || '-v' || version
                AND (NOT EXISTS (SELECT 1 FROM parts p WHERE p.recipe_revision=s.revision_id AND p.source='offlineCandidate')
                    OR EXISTS (SELECT 1 FROM parts p WHERE p.recipe_revision=s.revision_id AND p.source!='offlineCandidate'))) GROUP BY recipe_id").map_err(db_err)?;
        let rows = stmt.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))).map_err(db_err)?;
        rows.collect::<Result<std::collections::BTreeMap<_, _>, _>>().map_err(db_err)
    }

    #[cfg(test)]
    pub fn reserve_cycle_id(&self) -> Result<String, String> {
        Ok(self.reserve_cycle_ids(1)?.remove(0))
    }

    pub(crate) fn workspace_instance(&self, recipe_id: &str, instance_id: u64) -> Result<u64, String> {
        let conn = self.conn.lock().unwrap();
        if instance_id == 0 {
            let allocated: i64 = conn.query_row("INSERT INTO workspace_instances (recipe_id) VALUES (?1) RETURNING id", [recipe_id], |row| row.get(0)).map_err(db_err)?;
            return u64::try_from(allocated).map_err(|_| "候选实例 ID 无效".into());
        }
        let stored = i64::try_from(instance_id).map_err(|_| "候选实例 ID 超出范围")?;
        conn.execute("INSERT OR IGNORE INTO workspace_instances (id, recipe_id) VALUES (?1, ?2)", params![stored, recipe_id]).map_err(db_err)?;
        let original: String = conn.query_row("SELECT recipe_id FROM workspace_instances WHERE id = ?1", [stored], |row| row.get(0)).map_err(db_err)?;
        if original != recipe_id { return Err("候选实例 ID 已属于其他配方".into()); }
        Ok(instance_id)
    }

    pub fn reserve_cycle_ids(&self, count: usize) -> Result<Vec<String>, String> {
        if !(1..=32).contains(&count) { return Err("工件身份批量分配数量必须为 1–32".into()); }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let ids = (0..count).map(|_| tx.query_row(
            "INSERT INTO cycle_ids (created_at) VALUES (?1) RETURNING id",
            [ly_plc::now_ms()], |row| row.get(0),
        ).map_err(db_err)).collect::<Result<Vec<String>, String>>()?;
        tx.commit().map_err(db_err)?;
        Ok(ids)
    }

    pub fn insert(&self, r: &PartRecord) -> Result<i64, String> {
        if r.cycle_id.is_some_and(|s| s.is_empty()) || r.bundle_id.is_some_and(|s| s.is_empty()) {
            return Err("检测记录的 cycleId 或发布包 ID 为空".into());
        }
        if let Some(recipe) = r.recipe {
            let valid_revision = if r.source == RecordSource::OfflineCandidate { recipe.candidate_revision().is_some() }
                else { recipe.revision_id == format!("{}-v{}", recipe.id, recipe.version) };
            if !valid_revision || recipe.version == 0 {
                return Err("配方修订号与实际 ID、版本不一致".into());
            }
            if let Some(burst) = r.burst {
                validate_burst(r, recipe, burst)?;
            } else if r.shots.len() != recipe.shot_count() || r.frames_expected != recipe.shot_count() {
                return Err("检测记录必须包含每个计划拍照点，包括缺帧拍照点".into());
            }
            let mut seen = std::collections::HashSet::new();
            for shot in r.shots {
                let expected = recipe.shots.get(shot.k).ok_or("记录中的拍照点序号越界")?;
                if !seen.insert(shot.k) || shot.shot_id != expected.id || shot.camera != expected.camera || shot.view != expected.view {
                    return Err("记录中的拍照点身份、相机或视角与原配方不一致".into());
                }
            }
            if r.table.is_some_and(|t| t.len() != recipe.point_count()) {
                return Err("测量点数据长度与原配方不一致".into());
            }
        } else if !r.shots.is_empty() || r.table.is_some() || r.burst.is_some() {
            return Err("缺少原配方，不能保存无身份的拍照点、测量点或帧记录".into());
        }
        if r.sample.is_some() && !matches!(r.source, RecordSource::OfflineSample | RecordSource::OfflineCandidate) {
            return Err("只有离线·样本集的记录可以关联样本件".into());
        }
        for shot in r.shots {
            if shot.score.is_some_and(|x| !x.is_finite()) {
                return Err("拍照点得分不是有限数".into());
            }
            validate_raw_files(&shot.raw_files)?;
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        if let Some(recipe) = r.recipe {
            let json = serde_json::to_string(recipe).map_err(|e| e.to_string())?;
            let previous: Option<(String, u32, String)> = tx.query_row(
                "SELECT recipe_id,version,json FROM recipe_snapshots WHERE revision_id = ?1", [&recipe.revision_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional().map_err(db_err)?;
            if let Some((id, version, previous)) = previous {
                if id != recipe.id || version != recipe.version {
                    return Err("原配方快照 ID 或版本损坏，拒绝追加记录".into());
                }
                let previous = Recipe::from_snapshot(serde_json::from_str(&previous).map_err(|e| e.to_string())?)?;
                if serde_json::to_value(&previous).map_err(|e| e.to_string())? != serde_json::to_value(recipe).map_err(|e| e.to_string())? {
                    return Err("相同配方修订号对应了不同快照，拒绝覆盖原记录".into());
                }
            }
            tx.execute(
                "INSERT OR IGNORE INTO recipe_snapshots (revision_id, recipe_id, version, json) VALUES (?1, ?2, ?3, ?4)",
                params![recipe.revision_id, recipe.id, recipe.version, json],
            )
            .map_err(db_err)?;
        }
        let retest_of: Option<i64> = if r.sn.is_empty() {
            None
        } else {
            tx.query_row("SELECT id FROM parts WHERE sn = ?1 ORDER BY id DESC LIMIT 1", [r.sn], |row| row.get(0)).optional().map_err(db_err)?
        };
        let j = r.judgement;
        let text = |value: Result<String, serde_json::Error>| value.map_err(|e| e.to_string());
        let burst_columns = match r.burst {
            Some(burst) => (
                burst.expected_total,
                Some(burst.tolerance_total),
                burst.device_triggers,
                Some(text(serde_json::to_string(&burst.camera_frames))?),
                Some(text(serde_json::to_string(&burst.judgement.flags))?),
                Some(text(serde_json::to_string(&burst.judgement))?),
            ),
            None => (None, None, None, None, None, None),
        };
        tx.execute(
            "INSERT INTO parts (ts, sn, recipe_id, recipe_version, recipe_revision, trigger_mode, verdict, plc_code, fault_code,
                 reason, drain_ms, frames_expected, frames_received, triggers, retest_of, software_version, judgement, frames,
                 cycle_id, bundle_id, delivery_state, delivery_updated_at, delivery_message, sample, source,
                 expected_total, tolerance_total, device_triggers, camera_frames, flags, burst_judgement, mode)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18,
                 ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32)",
            params![
                r.ts,
                r.sn,
                r.recipe.map(|x| x.id.clone()),
                r.recipe.map(|x| x.version),
                r.recipe.map(|x| x.revision_id.clone()),
                r.recipe.and_then(|x| serde_json::to_value(x.trigger_mode).ok().and_then(|v| v.as_str().map(String::from))),
                verdict_str(j.verdict),
                j.plc_code,
                j.fault_code,
                j.reason,
                r.drain_ms.map(|v| v as i64),
                r.frames_expected as i64,
                r.frames_received as i64,
                r.triggers as i64,
                retest_of,
                r.software_version,
                serde_json::to_string(j).map_err(|e| e.to_string())?,
                serde_json::to_string(r.frames).map_err(|e| e.to_string())?,
                r.cycle_id,
                r.bundle_id,
                enum_name(r.delivery.state),
                r.delivery.updated_at,
                r.delivery.message,
                r.sample.map(serde_json::to_string).transpose().map_err(|e| e.to_string())?,
                enum_name(r.source),
                burst_columns.0,
                burst_columns.1,
                burst_columns.2,
                burst_columns.3,
                burst_columns.4,
                burst_columns.5,
                enum_name(r.mode),
            ],
        )
        .map_err(db_err)?;
        let id = tx.last_insert_rowid();
        if let Some(burst) = r.burst {
            let mut insert = tx.prepare(
                "INSERT INTO part_frames (part_id,camera,seq,status,frame_counter,trigger_counter,region,metrics,raw_file)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            ).map_err(db_err)?;
            for frame in &burst.frames {
                insert.execute(params![
                    id,
                    frame.camera as i64,
                    frame.seq as i64,
                    enum_name(frame.status),
                    frame.frame_counter.map(|n| n.to_string()),
                    frame.trigger_counter.map(|n| n.to_string()),
                    frame.region.map(|n| n as i64),
                    frame.metrics.as_ref().map(serde_json::to_string).transpose().map_err(|e| e.to_string())?,
                    frame.raw_file,
                ]).map_err(db_err)?;
            }
        }
        for shot in r.shots {
            tx.execute(
                "INSERT INTO part_shots (part_id,k,shot_id,camera,view,session,ordinal,frame_counter,trigger_counter,status,error,score,ms,raw_files)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
                params![
                    id,
                    shot.k as i64,
                    shot.shot_id,
                    shot.camera,
                    shot.view,
                    shot.session.map(|n| n.to_string()),
                    shot.ordinal.map(|n| n.to_string()),
                    shot.frame_counter.map(|n| n.to_string()),
                    shot.trigger_counter.map(|n| n.to_string()),
                    enum_name(shot.status),
                    shot.error,
                    shot.score,
                    shot.ms,
                    serde_json::to_string(&shot.raw_files).map_err(|e| e.to_string())?
                ],
            )
            .map_err(db_err)?;
        }
        if let Some(table) = r.table {
            tx.execute("INSERT INTO part_points (part_id, format, data) VALUES (?1, ?2, ?3)", params![id, POINTS_FORMAT, encode(table)]).map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(id)
    }

    pub fn matches_record(&self, record: &PartRecord<'_>) -> Result<bool, String> {
        let cycle = record.cycle_id.ok_or("审计重放缺少 cycleId")?;
        let Some(existing) = self.detail_by_cycle(cycle)? else { return Ok(false); };
        let summary = &existing.summary;
        let actual_recipe = match summary.recipe_revision.as_deref() {
            Some(revision) => self.recipe_snapshot(revision)?,
            None => None,
        };
        let mut actual_shots = existing.shots;
        let mut expected_shots = record.shots.to_vec();
        for shot in actual_shots.iter_mut().chain(expected_shots.iter_mut()) { shot.raw_files.clear(); }
        let consistent = summary.ts == record.ts && summary.sn == record.sn
            && summary.cycle_id.as_deref() == record.cycle_id && summary.bundle_id.as_deref() == record.bundle_id
            && summary.recipe_id.as_deref() == record.recipe.map(|recipe| recipe.id.as_str())
            && summary.recipe_version == record.recipe.map(|recipe| recipe.version)
            && summary.recipe_revision.as_deref() == record.recipe.map(|recipe| recipe.revision_id.as_str())
            && summary.frames_expected == record.frames_expected && summary.frames_received == record.frames_received
            && summary.sample.as_ref() == record.sample && summary.source == record.source
            && existing.triggers == record.triggers && existing.software_version == record.software_version
            && serde_json::to_value(&existing.judgement).map_err(|error| error.to_string())? == serde_json::to_value(record.judgement).map_err(|error| error.to_string())?
            && serde_json::to_value(&existing.frames).map_err(|error| error.to_string())? == serde_json::to_value(record.frames).map_err(|error| error.to_string())?
            && serde_json::to_value(&actual_shots).map_err(|error| error.to_string())? == serde_json::to_value(&expected_shots).map_err(|error| error.to_string())?
            && serde_json::to_value(actual_recipe).map_err(|error| error.to_string())? == serde_json::to_value(record.recipe).map_err(|error| error.to_string())?
            && same_burst(existing.burst.as_ref(), record.burst)?;
        if !consistent { return Ok(false); }
        let conn = self.conn.lock().unwrap();
        let actual_points = conn.query_row("SELECT format,data FROM part_points WHERE part_id=?1", [summary.id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))).optional().map_err(db_err)?;
        Ok(match (actual_points, record.table) {
            (None, None) => true,
            (Some((POINTS_FORMAT, data)), Some(table)) => data == encode(table),
            _ => false,
        })
    }

    pub fn update_submission_timing(&self, cycle_id: &str, drain_ms: u64) -> Result<bool, String> {
        let value = i64::try_from(drain_ms).map_err(|_| "PLC 提交耗时超出存储范围")?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let previous = tx.query_row("SELECT drain_ms FROM parts WHERE cycle_id=?1", [cycle_id], |row| row.get::<_, Option<i64>>(0))
            .optional().map_err(db_err)?;
        let Some(previous) = previous else { return Ok(false); };
        if let Some(previous) = previous {
            if previous != value { return Err("PLC 提交耗时已确认，不能替换原始观测".into()); }
        } else {
            tx.execute("UPDATE parts SET drain_ms=?2 WHERE cycle_id=?1", params![cycle_id, value]).map_err(db_err)?;
        }
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    pub fn update_delivery(&self, cycle_id: &str, delivery: &PlcDelivery) -> Result<bool, String> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let previous = tx
            .query_row("SELECT delivery_state,delivery_updated_at,delivery_message FROM parts WHERE cycle_id=?1", [cycle_id], |r| delivery_row(r, 0))
            .optional()
            .map_err(db_err)?;
        let Some(previous) = previous else { return Ok(false) };
        if delivery.updated_at < previous.updated_at {
            return Ok(false);
        }
        if previous.state == PlcDeliveryState::Acknowledged && delivery.state != PlcDeliveryState::Acknowledged {
            return Err("PLC 已确认交付，不能回退交付状态".into());
        }
        if previous.state == PlcDeliveryState::NotRequired && delivery.state != PlcDeliveryState::NotRequired {
            return Err("该记录无需 PLC 交付，不能追加到其他事务".into());
        }
        if previous.state != PlcDeliveryState::NotRequired && delivery.state == PlcDeliveryState::NotRequired {
            return Err("已有 PLC 交付事务，不能改为无需交付".into());
        }
        if matches!(previous.state, PlcDeliveryState::Submitted | PlcDeliveryState::Failed) && delivery.state == PlcDeliveryState::Pending {
            return Err("PLC 交付已尝试，不能回退为待提交".into());
        }
        tx.execute(
            "UPDATE parts SET delivery_state=?2,delivery_updated_at=?3,delivery_message=?4 WHERE cycle_id=?1",
            params![cycle_id, enum_name(delivery.state), delivery.updated_at, delivery.message],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    pub fn unresolved_delivery_cycles(&self) -> Result<BTreeSet<String>, String> {
        let conn = self.conn.lock().unwrap();
        let mut statement = conn.prepare("SELECT cycle_id FROM parts WHERE cycle_id IS NOT NULL AND delivery_state IN ('pending','submitted','failed')").map_err(db_err)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(db_err)?;
        rows.collect::<Result<BTreeSet<_>, _>>().map_err(db_err)
    }

    pub fn recover_acknowledgement(&self, cycle_id: &str, sn: &str, request_seq: u32, acknowledged_at: i64) -> Result<bool, String> {
        if cycle_id.len() != 32 || !cycle_id.bytes().all(|b| b.is_ascii_hexdigit()) || request_seq == 0 || acknowledged_at < 0 {
            return Err("PLC 确认收据的身份无效".into());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate).map_err(db_err)?;
        let previous = tx.query_row(
            "SELECT sn,delivery_state,delivery_updated_at,delivery_message FROM parts WHERE cycle_id=?1",
            [cycle_id], |r| Ok((r.get::<_, String>(0)?, delivery_row(r, 1)?)),
        ).optional().map_err(db_err)?;
        let Some((recorded_sn, previous)) = previous else { return Err("PLC 已确认，但对应 cycleId 的检测记录不存在；保留握手审计供核查".into()) };
        if recorded_sn != sn { return Err("PLC 确认收据与检测记录的 SN 不一致".into()); }
        if previous.state == PlcDeliveryState::NotRequired { return Err("检测记录无需 PLC 交付，不能追加确认收据".into()); }
        if previous.state == PlcDeliveryState::Acknowledged { return Ok(false); }
        tx.execute("UPDATE parts SET delivery_state='acknowledged',delivery_updated_at=?2,delivery_message=?3 WHERE cycle_id=?1",
            params![cycle_id, previous.updated_at.max(acknowledged_at), format!("从持久握手记录恢复 PLC 确认：请求序号 {request_seq}，收据时间 {acknowledged_at}")])
            .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    pub fn update_shot_raw_files(&self, cycle_id: &str, k: usize, files: &[ShotRawFile]) -> Result<bool, String> {
        validate_raw_files(files)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let previous: Option<(i64, Vec<ShotRawFile>)> = tx
            .query_row(
                "SELECT s.part_id,s.raw_files FROM part_shots s JOIN parts p ON p.id=s.part_id WHERE p.cycle_id=?1 AND s.k=?2",
                params![cycle_id, k as i64],
                |row| Ok((row.get(0)?, json_column(row, 1)?)),
            )
            .optional()
            .map_err(db_err)?;
        let Some((id, previous)) = previous else { return Ok(false) };
        if previous.iter().any(|old| !files.iter().any(|new| old.view == new.view && old.file == new.file
            && (old.width.is_none() || old.width == new.width) && (old.height.is_none() || old.height == new.height))) {
            return Err("原图身份已保存，不能替换为另一组文件".into());
        }
        tx.execute(
            "UPDATE part_shots SET raw_files=?3 WHERE part_id=?1 AND k=?2",
            params![id, k as i64, serde_json::to_string(files).map_err(|e| e.to_string())?],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    /// 段配方记录补原图引用：按 (camera, seq) 只增不换——已保存的引用不能改成另一个文件，重复写同一文件是幂等的。
    /// 一批里有不存在的记录或帧时整批不写，返回 false。
    pub fn update_frame_raw_files(&self, cycle_id: &str, files: &std::collections::BTreeMap<FrameKey, String>) -> Result<bool, String> {
        for file in files.values() {
            validate_raw_path(file)?;
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let Some(id) = tx.query_row("SELECT id FROM parts WHERE cycle_id=?1", [cycle_id], |row| row.get::<_, i64>(0)).optional().map_err(db_err)? else {
            return Ok(false);
        };
        for (key, file) in files {
            let seq = i64::try_from(key.seq).map_err(|_| "帧组序号超出存储范围")?;
            let previous: Option<Option<String>> = tx
                .query_row("SELECT raw_file FROM part_frames WHERE part_id=?1 AND camera=?2 AND seq=?3", params![id, key.camera as i64, seq], |row| row.get(0))
                .optional()
                .map_err(db_err)?;
            match previous {
                None => return Ok(false),
                Some(Some(previous)) if previous != *file => {
                    return Err(format!("相机 {} 第 {} 帧的原图身份已保存，不能替换为另一个文件", key.camera, key.seq));
                }
                Some(Some(_)) => (),
                Some(None) => {
                    tx.execute("UPDATE part_frames SET raw_file=?4 WHERE part_id=?1 AND camera=?2 AND seq=?3", params![id, key.camera as i64, seq, file])
                        .map_err(db_err)?;
                }
            }
        }
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    pub fn update_recording(&self, cycle_id: &str, evidence: &RecordingEvidence) -> Result<bool, String> {
        if evidence.available != (evidence.state == RecordingState::Complete)
            || (evidence.available && (!evidence.errors.is_empty() || evidence.directory.as_deref().is_none_or(str::is_empty)))
        {
            return Err("只有带实际目录且无错误的完整录制才能标记原图可用".into());
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction().map_err(db_err)?;
        let previous: Option<(i64, RecordingEvidence)> = tx
            .query_row("SELECT id,recording_state,recording_available,recording_directory,recording_errors FROM parts WHERE cycle_id=?1", [cycle_id], |row| {
                Ok((row.get(0)?, recording_row(row, 1)?))
            })
            .optional()
            .map_err(db_err)?;
        let Some((id, previous)) = previous else { return Ok(false) };
        if previous.state != RecordingState::Pending && previous != *evidence {
            return Err("原录制收尾证据已保存，不能替换或回退".into());
        }
        if evidence.available {
            let mut stmt = tx.prepare("SELECT raw_files FROM part_shots WHERE part_id=?1").map_err(db_err)?;
            let refs = stmt.query_map([id], |row| json_column::<Vec<ShotRawFile>>(row, 0)).map_err(db_err)?;
            let mut has_file = false;
            for files in refs {
                let files = files.map_err(db_err)?;
                validate_raw_files(&files)?;
                has_file |= !files.is_empty();
            }
            has_file |= tx
                .query_row("SELECT EXISTS(SELECT 1 FROM part_frames WHERE part_id=?1 AND raw_file IS NOT NULL)", [id], |row| row.get::<_, bool>(0))
                .map_err(db_err)?;
            if !has_file {
                return Err("没有成功保存的原图引用，不能标记录制可用".into());
            }
        }
        tx.execute(
            "UPDATE parts SET recording_state=?2,recording_available=?3,recording_directory=?4,recording_errors=?5 WHERE id=?1",
            params![id, enum_name(evidence.state), evidence.available, evidence.directory, serde_json::to_string(&evidence.errors).map_err(|e| e.to_string())?],
        )
        .map_err(db_err)?;
        tx.commit().map_err(db_err)?;
        Ok(true)
    }

    fn filter(q: &HistoryQuery, prefix: &str) -> (String, Vec<SqlValue>) {
        let mut clauses = vec![q.scope.clause(prefix)];
        let mut args = Vec::new();
        if let Some(from) = q.from {
            clauses.push(format!("{prefix}ts >= ?"));
            args.push(SqlValue::Integer(from));
        }
        if let Some(to) = q.to {
            clauses.push(format!("{prefix}ts < ?"));
            args.push(SqlValue::Integer(to));
        }
        if !q.verdicts.is_empty() {
            clauses.push(format!("{prefix}verdict IN ({})", vec!["?"; q.verdicts.len()].join(",")));
            args.extend(q.verdicts.iter().map(|v| SqlValue::Text(verdict_str(*v))));
        }
        if let Some(sn) = q.sn.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            clauses.push(format!("{prefix}sn LIKE ? ESCAPE '\\'"));
            args.push(SqlValue::Text(format!("%{}%", sn.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"))));
        }
        if let Some(id) = q.recipe_id.as_deref().filter(|s| !s.is_empty()) {
            clauses.push(format!("{prefix}recipe_id = ?"));
            args.push(SqlValue::Text(id.to_string()));
        }
        if !q.modes.is_empty() {
            clauses.push(format!(
                "{prefix}mode IN ({})",
                vec!["?"; q.modes.len()].join(",")
            ));
            args.extend(q.modes.iter().map(|mode| SqlValue::Text(enum_name(*mode))));
        }
        (format!(" WHERE {}", clauses.join(" AND ")), args)
    }

    /// 某个录制根目录下仍被历史引用的 _pending 目录；只看同一范围的记录，真机与离线各管各的根目录。
    pub fn pending_recording_directories(&self, root: &Path, scope: HistoryScope) -> Result<Vec<PathBuf>, String> {
        let root_text = root.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_string();
        let prefix = format!("{root_text}/");
        let mut paths = Vec::new();
        let mut protect = |value: &str| -> Result<(), String> {
            let normalized = value.replace('\\', "/");
            let relative = if normalized.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(&prefix)) {
                &normalized[prefix.len()..]
            } else if !Path::new(value).is_absolute() && !normalized.contains(':') {
                normalized.as_str()
            } else {
                if normalized.to_ascii_lowercase().contains("/_pending/") {
                    return Err("历史 pending 引用不在当前 records 根目录，暂停清理".into());
                }
                return Ok(());
            };
            let components: Vec<_> = relative.split('/').collect();
            if !components.first().is_some_and(|p| p.eq_ignore_ascii_case("_pending")) { return Ok(()); }
            if components.len() < 2 || components.iter().any(|p| p.is_empty() || *p == "." || *p == ".." || p.contains(':')) {
                return Err("历史 pending 录制引用路径无效，暂停清理".into());
            }
            paths.push(root.join("_pending").join(components[1]));
            Ok(())
        };
        let conn = self.conn.lock().map_err(|_| "数据库锁异常，暂停孤立录制清理".to_string())?;
        let mut stmt = conn.prepare(&format!("SELECT recording_directory FROM parts WHERE instr(lower(recording_directory),'_pending')>0 AND {}", scope.clause(""))).map_err(db_err)?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0)).map_err(db_err)? {
            protect(&row.map_err(db_err)?)?;
        }
        let mut stmt = conn.prepare(&format!("SELECT s.raw_files FROM part_shots s JOIN parts p ON p.id=s.part_id \
            WHERE (NOT json_valid(s.raw_files) OR instr(lower(s.raw_files),'_pending')>0) AND {}", scope.clause("p."))).map_err(db_err)?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0)).map_err(db_err)? {
            let files: Vec<ShotRawFile> = serde_json::from_str(&row.map_err(db_err)?).map_err(|e| format!("历史原图引用损坏，暂停清理：{e}"))?;
            validate_raw_files(&files)?;
            for file in files { protect(&file.file)?; }
        }
        let mut stmt = conn.prepare(&format!("SELECT f.raw_file FROM part_frames f JOIN parts p ON p.id=f.part_id \
            WHERE instr(lower(f.raw_file),'_pending')>0 AND {}", scope.clause("p."))).map_err(db_err)?;
        for row in stmt.query_map([], |row| row.get::<_, String>(0)).map_err(db_err)? {
            let file = row.map_err(db_err)?;
            validate_raw_path(&file)?;
            protect(&file)?;
        }
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    /// 滚动清理即将删除这些录制目录：引用它们的记录改为“按保留策略不存图”并写明原因，不再显示“已核验落盘”。
    pub fn release_recordings(&self, directories: &[PathBuf]) -> Result<(), String> {
        let note = "原图已按录制保留件数或容量上限滚动清理";
        let directories: Vec<_> = directories.iter().map(|directory| directory.to_string_lossy()).collect();
        let directories = serde_json::to_string(&directories).map_err(|e| e.to_string())?;
        let conn = self.conn.lock().map_err(|_| "数据库锁异常，暂停滚动清理".to_string())?;
        conn.execute("UPDATE parts SET recording_state=?3, recording_available=0, recording_errors=json_insert(recording_errors,'$[#]',?2) \
            WHERE recording_directory IN (SELECT value FROM json_each(?1)) AND recording_state IN ('complete','incomplete')",
            params![directories, note, enum_name(RecordingState::NotRetained)]).map_err(db_err)?;
        Ok(())
    }

    pub fn query(&self, q: &HistoryQuery) -> Result<HistoryPage, String> {
        let conn = self.conn.lock().unwrap();
        let (filter, args) = Self::filter(q, "");
        let mut counts = VerdictCounts::default();
        let mut total = 0;
        let mut stmt = conn.prepare(&format!("SELECT verdict, COUNT(*) FROM parts{filter} GROUP BY verdict")).map_err(db_err)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))).map_err(db_err)?;
        for row in rows {
            let (v, n) = row.map_err(db_err)?;
            counts.add(parse_verdict(&v), n);
            total += n;
        }
        let limit = if q.limit == 0 { 50 } else { q.limit.min(500) };
        let sql = format!("SELECT {SUMMARY_COLS} FROM parts{filter} ORDER BY id DESC LIMIT {limit} OFFSET {}", q.offset);
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let items = stmt.query_map(params_from_iter(args.iter()), summary).map_err(db_err)?.collect::<Result<_, _>>().map_err(db_err)?;
        Ok(HistoryPage { total, counts, items })
    }

    /// 同一筛选条件下与 id 相邻的记录：(较早一件, 较晚一件)。
    pub fn neighbors(&self, q: &HistoryQuery, id: i64) -> Result<(Option<i64>, Option<i64>), String> {
        let conn = self.conn.lock().unwrap();
        let (filter, args) = Self::filter(q, "");
        let one = |cmp: &str, order: &str| -> Result<Option<i64>, String> {
            let mut values = args.clone();
            values.push(SqlValue::Integer(id));
            conn.query_row(&format!("SELECT id FROM parts{filter} AND id {cmp} ? ORDER BY id {order} LIMIT 1"),
                params_from_iter(values.iter()), |row| row.get(0)).optional().map_err(db_err)
        };
        Ok((one("<", "DESC")?, one(">", "ASC")?))
    }

    pub fn detail(&self, id: i64) -> Result<PartDetail, String> {
        let conn = self.conn.lock().unwrap();
        const N: usize = SUMMARY_LEN;
        let (summary, judgement, frames, triggers, software_version, recording, burst_judgement) = conn
            .query_row(
                &format!("SELECT {SUMMARY_COLS}, judgement, frames, triggers, software_version,recording_state,recording_available,recording_directory,recording_errors,burst_judgement FROM parts WHERE id = ?1"),
                [id],
                |row| Ok((summary(row)?, row.get::<_, String>(N)?, row.get::<_, String>(N + 1)?, row.get::<_, i64>(N + 2)? as u64, row.get::<_, String>(N + 3)?,
                    recording_row(row, N + 4)?, row.get::<_, Option<String>>(N + 8)?)),
            )
            .optional()
            .map_err(db_err)?
            .ok_or_else(|| format!("记录 {id} 不存在或已过期清理"))?;
        let burst = match (burst_judgement, &summary.burst) {
            (Some(judgement), Some(counts)) => {
                let revision = summary.recipe_revision.as_deref().ok_or_else(|| format!("记录 {id} 是段配方记录却没有配方快照身份"))?;
                let recipe = snapshot_in(&conn, revision)?.ok_or_else(|| format!("记录 {id} 的原配方快照缺失"))?;
                Some(PartDetailBurst {
                    segments: recipe.path_segments.clone(),
                    cameras: recipe.plan_cameras(),
                    expected_total: counts.expected_total,
                    tolerance_total: counts.tolerance_total,
                    device_triggers: counts.device_triggers,
                    camera_frames: counts.camera_frames.clone(),
                    judgement: serde_json::from_str(&judgement).map_err(|e| format!("记录 {id} 的区域判定损坏：{e}"))?,
                    frames: frame_rows(&conn, id)?,
                })
            }
            (None, None) => None,
            _ => return Err(format!("记录 {id} 的段配方字段不完整")),
        };
        let points = conn
            .query_row("SELECT format, data FROM part_points WHERE part_id = ?1", [id], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)))
            .optional()
            .map_err(db_err)?
            .map(|(format, blob)| -> Result<PartPoints, String> {
                let raw = raw_points(format, &blob)?;
                Ok(PartPoints {
                    d: raw.iter().map(|p| (p.2 == ST_MEASURED).then_some(p.0)).collect(),
                    w: raw.iter().map(|p| p.1.is_finite().then_some(p.1)).collect(),
                    st: raw.iter().map(|p| p.2).collect(),
                    table: raw.iter().map(|p| point_state(p.0, p.1, p.2)).collect::<Result<_, _>>()?,
                })
            })
            .transpose()?;
        let mut stmt = conn.prepare("SELECT id FROM parts WHERE retest_of = ?1 ORDER BY id").map_err(db_err)?;
        let retests = stmt.query_map([id], |row| row.get(0)).map_err(db_err)?.collect::<Result<_, _>>().map_err(db_err)?;
        let mut stmt = conn
            .prepare(
                "SELECT k,shot_id,camera,view,session,ordinal,frame_counter,trigger_counter,status,error,score,ms,raw_files
            FROM part_shots WHERE part_id=?1 ORDER BY k",
            )
            .map_err(db_err)?;
        let shots = stmt
            .query_map([id], |row| {
                Ok(PartShot {
                    k: row.get::<_, i64>(0)? as usize,
                    shot_id: row.get(1)?,
                    camera: row.get(2)?,
                    view: row.get(3)?,
                    session: count_column(row, 4)?,
                    ordinal: count_column(row, 5)?,
                    frame_counter: count_column(row, 6)?,
                    trigger_counter: count_column(row, 7)?,
                    status: enum_column(row, 8)?,
                    error: row.get(9)?,
                    score: row.get(10)?,
                    ms: row.get(11)?,
                    raw_files: json_column(row, 12)?,
                })
            })
            .map_err(db_err)?
            .collect::<Result<_, _>>()
            .map_err(db_err)?;
        Ok(PartDetail {
            summary,
            judgement: serde_json::from_str(&judgement).map_err(|e| e.to_string())?,
            frames: serde_json::from_str(&frames).map_err(|e| format!("记录 {id} 的帧信息损坏：{e}"))?,
            triggers,
            software_version,
            points,
            retests,
            shots,
            recording,
            burst,
        })
    }

    pub fn detail_by_cycle(&self, cycle_id: &str) -> Result<Option<PartDetail>, String> {
        let id =
            self.conn.lock().unwrap().query_row("SELECT id FROM parts WHERE cycle_id=?1", [cycle_id], |row| row.get::<_, i64>(0)).optional().map_err(db_err)?;
        id.map(|id| self.detail(id)).transpose()
    }

    pub fn recipe_snapshot(&self, revision_id: &str) -> Result<Option<Recipe>, String> {
        snapshot_in(&self.conn.lock().unwrap(), revision_id)
    }

    /// 段配方记录的逐帧记录，按 (camera, seq) 排序；旧记录为空。
    pub fn burst_frames(&self, id: i64) -> Result<Vec<FrameRecord>, String> {
        frame_rows(&self.conn.lock().unwrap(), id)
    }

    /// 取出重判候选，缺少测量数据的记录返回空表；ids 为空时按筛选条件。
    pub fn measurements(&self, q: &HistoryQuery, ids: &[i64], limit: usize) -> Result<Vec<StoredMeasurement>, String> {
        let conn = self.conn.lock().unwrap();
        let (filter, args) = if ids.is_empty() {
            Self::filter(q, "p.")
        } else {
            (format!(" WHERE p.id IN ({})", vec!["?"; ids.len()].join(",")), ids.iter().map(|&i| SqlValue::Integer(i)).collect())
        };
        let sql = format!(
            "SELECT p.id, p.ts, p.sn, p.recipe_id, p.recipe_revision, p.verdict, COALESCE(pp.format, {POINTS_FORMAT}), COALESCE(pp.data, x''), p.burst_judgement FROM parts p
             LEFT JOIN part_points pp ON pp.part_id = p.id{filter} ORDER BY p.id DESC LIMIT {limit}"
        );
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map(params_from_iter(args.iter()), |row| {
                Ok(StoredMeasurement {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    sn: row.get(2)?,
                    recipe_id: row.get(3)?,
                    recipe_revision: row.get(4)?,
                    verdict: parse_verdict(&row.get::<_, String>(5)?),
                    table: decode(row.get(6)?, &row.get::<_, Vec<u8>>(7)?).map_err(|error| {
                        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Blob, error.into())
                    })?,
                    burst: row.get::<_, Option<String>>(8)?.map(|text| serde_json::from_str(&text)
                        .map_err(|e| rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e)))).transpose()?,
                })
            })
            .map_err(db_err)?;
        rows.collect::<Result<_, _>>().map_err(db_err)
    }

    pub fn collection_progress(
        &self,
        scope: HistoryScope,
    ) -> Result<Vec<CollectionProgress>, String> {
        let conn = self.conn.lock().unwrap();
        let sql = format!("SELECT recipe_id, recording_available, camera_frames FROM parts WHERE {} AND mode = 'maintenanceCollect' \
            AND recipe_id IS NOT NULL AND recording_directory IS NOT NULL ORDER BY recipe_id", scope.clause(""));
        let mut stmt = conn.prepare(&sql).map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, bool>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(db_err)?;
        let mut progress: Vec<CollectionProgress> = Vec::new();
        for row in rows {
            let (recipe_id, available, frames) = row.map_err(db_err)?;
            let frames: Vec<u32> = frames
                .and_then(|text| serde_json::from_str(&text).ok())
                .unwrap_or_default();
            let usable = available
                && frames
                    .first()
                    .is_some_and(|&n| n > 0 && frames.iter().all(|&m| m == n));
            if progress
                .last()
                .is_none_or(|last| last.recipe_id != recipe_id)
            {
                progress.push(CollectionProgress {
                    recipe_id,
                    ..CollectionProgress::default()
                });
            }
            let last = progress.last_mut().unwrap();
            last.recorded += 1;
            last.usable += u32::from(usable);
        }
        Ok(progress)
    }

    pub fn counts_since(&self, since: i64, scope: HistoryScope) -> Result<VerdictCounts, String> {
        self.query(&HistoryQuery {
            from: Some(since),
            scope,
            modes: vec![PartMode::Production],
            limit: 1,
            ..Default::default()
        })
        .map(|p| p.counts)
    }

    pub(crate) fn comparison_cycle(&self, id: i64) -> Result<Option<String>, String> {
        self.conn.lock().unwrap().query_row(
            "SELECT COALESCE(cycle_id, '') FROM parts WHERE id = ?1", [id], |row| row.get(0)
        ).optional().map_err(db_err)
    }

    pub fn purge_before(&self, ts: i64) -> Result<usize, String> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM parts WHERE ts < ?1", [ts]).map_err(db_err)
    }
}

#[cfg(test)]
#[path = "store/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "store/burst_fixture.rs"]
pub(crate) mod burst_fixture;
