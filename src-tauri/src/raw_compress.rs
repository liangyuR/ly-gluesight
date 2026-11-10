//! 历史件原图的空闲压缩（docs/decisions/image-storage.md IS-2）：件收尾、录制完整并入库后，系统连续空闲时把录制目录里的
//! `X.pgm` 逐帧压成同名的 `X.gsr`（`image_file::gsr`）。录制线程仍只写未压缩 PGM，读图入口透明还原。
//!
//! - 空闲：没有在途工件与未结 PLC 事务、录制队列为空、没有待入库的录制、没有登记的长任务（[`Activity::busy`]：导入导出、
//!   清理、复测、示教测量等），且最后一次业务活动（含读原图）之后连续满 [`QUIET`]。每帧之前重新检查。
//! - 压缩线程不持有任何锁：每帧开始时置“正在处理一帧”的标志再看有没有业务，业务登记时先计数再等这个标志清掉（最多一帧的时间），
//!   所以一帧的替换不会与登记了的清理、导出同时进行。
//! - 单帧：读 `X.pgm` → 编码 → 写 `X.gsr.tmp` 并落盘 → 读回解码、与原文件逐字节比较 → 改名为 `X.gsr` → 删 `X.pgm`。
//!   任一步失败保留 `X.pgm`、删临时文件。中断留下的 `.tmp` 与并存的 `X.pgm` / `X.gsr` 在下次处理这件时收拾（[`recover`]）。

use std::collections::HashSet;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::image_file::gsr;

/// 业务停止后连续空闲多久才开始压缩。
pub const QUIET: Duration = Duration::from_secs(60);
const POLL: Duration = Duration::from_secs(1);
/// 一轮扫完、其间没有新的业务活动时，隔多久再扫一轮（重试上一轮失败的帧）。
const REST: Duration = Duration::from_secs(30 * 60);
/// 一件里失败这么多帧就停下这一轮（盘满、无权限等不会逐帧好转）。
const PART_FAILURES: u32 = 3;
const PAGE: usize = 256;
/// “开始压缩”与“因业务暂停”来回切换时，两条状态日志最少隔多久。
const STATE_LOG: Duration = Duration::from_secs(5 * 60);
const TEMPORARY: &str = "gsr.tmp";
const EVENT: &str = "原图空闲压缩";

/// 业务活动的登记处：压缩线程据此判断空闲。
pub struct Activity {
    epoch: Instant,
    busy: AtomicUsize,
    in_frame: AtomicBool,
    /// 最后一次业务活动距 `epoch` 的毫秒数
    last: AtomicU64,
}

/// 进行中的业务（长任务、清理）；存在期间压缩线程不开始下一帧。
pub struct Busy<'a>(&'a Activity);

struct FramePermit<'a>(&'a Activity);

impl Default for Activity {
    fn default() -> Self {
        Self {
            epoch: Instant::now(),
            busy: AtomicUsize::new(0),
            in_frame: AtomicBool::new(false),
            last: AtomicU64::new(0),
        }
    }
}

impl Activity {
    fn now(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// 记一次业务活动（读原图、录制写盘、节拍在途）：空闲时间从这一刻重新算。
    pub fn touch(&self) {
        self.last.store(self.now(), Ordering::SeqCst);
    }

    /// 登记一项业务，返回的守卫存在期间不压缩。压缩线程正处理一个文件时等它做完才返回：拿到守卫就不会再有
    /// 压缩线程在动录制目录。一次许可只处理一个文件（一帧的压缩，或一个残留文件的收拾），等待以它的读写为限。
    pub fn busy(&self) -> Busy<'_> {
        self.busy.fetch_add(1, Ordering::SeqCst);
        self.touch();
        while self.in_frame.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Busy(self)
    }

    fn stamp(&self) -> u64 {
        self.last.load(Ordering::SeqCst)
    }

    fn quiet(&self, quiet: Duration) -> bool {
        self.busy.load(Ordering::SeqCst) == 0
            && self.now().saturating_sub(self.stamp()) >= quiet.as_millis() as u64
    }

    /// 压缩线程处理一帧之前取：有业务或空闲不满 `quiet` 时为 None。
    fn frame(&self, quiet: Duration) -> Option<FramePermit<'_>> {
        self.in_frame.store(true, Ordering::SeqCst);
        if self.quiet(quiet) {
            Some(FramePermit(self))
        } else {
            self.in_frame.store(false, Ordering::SeqCst);
            None
        }
    }
}

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.touch();
        self.0.busy.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Drop for FramePermit<'_> {
    fn drop(&mut self) {
        self.0.in_frame.store(false, Ordering::SeqCst);
    }
}

/// 本进程的业务活动登记处。
pub fn activity() -> &'static Activity {
    static ACTIVITY: OnceLock<Activity> = OnceLock::new();
    ACTIVITY.get_or_init(Activity::default)
}

static ENABLED: AtomicBool = AtomicBool::new(false);

/// 本机设置里的空闲压缩开关（`config/history.json` 的 `idleCompress`）。
pub fn set_enabled(enabled: bool) {
    ENABLED.store(enabled, Ordering::SeqCst);
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FrameOutcome {
    Compressed {
        before: u64,
        after: u64,
    },
    /// 压缩后不比原文件小，或不是能压缩的 8 位 P5 / P6：保留 `X.pgm`
    Kept,
    /// 文件或目录已不在（被清理）
    Gone,
    Failed(String),
}

fn plain_file(path: &Path) -> std::io::Result<bool> {
    let meta = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Ok(false);
        }
    }
    Ok(meta.is_file() && !meta.file_type().is_symlink())
}

fn temporary(pgm: &Path) -> PathBuf {
    pgm.with_extension(TEMPORARY)
}

/// 压缩一帧，见模块说明的单帧流程。
pub(crate) fn compress_frame(pgm: &Path) -> FrameOutcome {
    let gone = |error: &std::io::Error| error.kind() == ErrorKind::NotFound;
    match plain_file(pgm) {
        Ok(true) => {}
        Ok(false) => return FrameOutcome::Kept,
        Err(error) if gone(&error) => return FrameOutcome::Gone,
        Err(error) => return FrameOutcome::Failed(format!("读取失败：{error}")),
    }
    let original = match std::fs::read(pgm) {
        Ok(bytes) => bytes,
        Err(error) if gone(&error) => return FrameOutcome::Gone,
        Err(error) => return FrameOutcome::Failed(format!("读取失败：{error}")),
    };
    let Some(encoded) = gsr::encode(&original).filter(|encoded| encoded.len() < original.len())
    else {
        return FrameOutcome::Kept;
    };
    let temporary = temporary(pgm);
    let target = pgm.with_extension(gsr::EXTENSION);
    let written = std::fs::File::create(&temporary)
        .and_then(|mut file| {
            file.write_all(&encoded)?;
            file.sync_all()
        })
        .map_err(|error| format!("写临时文件失败：{error}"))
        .and_then(|()| {
            let stored = std::fs::read(&temporary).map_err(|e| format!("读回失败：{e}"))?;
            match gsr::decode(&stored) {
                Ok(restored) if restored == original => Ok(()),
                Ok(_) => Err("读回还原后与原文件不一致".to_string()),
                Err(error) => Err(format!("读回校验失败：{error}")),
            }
        })
        .and_then(|()| {
            std::fs::rename(&temporary, &target).map_err(|error| format!("改名失败：{error}"))
        });
    if let Err(error) = written {
        let _ = std::fs::remove_file(&temporary);
        // 目录被清理时写临时文件或改名会失败：原文件已不在，不算失败
        return if pgm.exists() {
            FrameOutcome::Failed(error)
        } else {
            FrameOutcome::Gone
        };
    }
    match std::fs::remove_file(pgm) {
        Ok(()) => FrameOutcome::Compressed {
            before: original.len() as u64,
            after: encoded.len() as u64,
        },
        Err(error) if gone(&error) => FrameOutcome::Gone,
        // 两个文件并存：读图仍用 X.pgm，下次处理这件时核对后删掉它
        Err(error) => FrameOutcome::Failed(format!("删除原文件失败：{error}")),
    }
}

fn entries(dir: &Path) -> Option<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    paths.sort();
    Some(paths)
}

fn has_extension(path: &Path, extension: &str) -> bool {
    path.extension().is_some_and(|e| e == extension)
}

/// 收拾中断留下的文件：`.gsr.tmp` 直接删；`X.pgm` 与 `X.gsr` 并存时，`X.gsr` 能还原出与 `X.pgm` 相同的字节就删 `X.pgm`，
/// 否则删 `X.gsr`。只有 `X.gsr` 的不动。每个要收拾的文件之前取一次 `permit`，取不到就停下。
/// 返回告警，以及是否因取不到许可停在半路。
pub(crate) fn recover<P>(dir: &Path, permit: impl Fn() -> Option<P>) -> (Vec<String>, bool) {
    let mut warnings = Vec::new();
    for path in entries(dir).unwrap_or_default() {
        let temporary = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(&format!(".{TEMPORARY}")));
        if !temporary && !has_extension(&path, gsr::EXTENSION) {
            continue;
        }
        let Some(_permit) = permit() else {
            return (warnings, true);
        };
        if temporary {
            if matches!(plain_file(&path), Ok(true)) {
                let _ = std::fs::remove_file(&path);
            }
            continue;
        }
        let pgm = path.with_extension("pgm");
        if !matches!(plain_file(&pgm), Ok(true)) || !matches!(plain_file(&path), Ok(true)) {
            continue;
        }
        let same = std::fs::read(&path)
            .ok()
            .and_then(|stored| gsr::decode(&stored).ok())
            .zip(std::fs::read(&pgm).ok())
            .is_some_and(|(restored, original)| restored == original);
        let (stale, kept) = if same { (&pgm, &path) } else { (&path, &pgm) };
        match std::fs::remove_file(stale) {
            Ok(()) if !same => warnings.push(format!(
                "{} 与原文件不一致，已删除并保留 {}",
                path.display(),
                kept.display()
            )),
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => warnings.push(format!("清理 {} 失败：{error}", stale.display())),
        }
    }
    (warnings, false)
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct PartStats {
    pub frames: u32,
    pub before: u64,
    pub after: u64,
    pub failed: u32,
    pub first_error: Option<String>,
    /// 业务恢复，停在半路
    pub interrupted: bool,
}

impl PartStats {
    fn merge(&mut self, other: PartStats) {
        self.frames += other.frames;
        self.before += other.before;
        self.after += other.after;
        self.failed += other.failed;
        self.first_error = self.first_error.take().or(other.first_error);
        self.interrupted = other.interrupted;
    }
}

/// 本次运行的记忆：处理完的件不再扫，没有收益的帧不再试（重启后各再试一次）。
#[derive(Default)]
pub(crate) struct Memo {
    done: HashSet<PathBuf>,
    kept: HashSet<PathBuf>,
}

/// 能压缩的录制目录：普通目录、已收尾（有 `part.json`、不在 `_pending` 里）、不是导入暂存或未提交的导入件，
/// 目录名是这件的 `*_cycle_<cycleId>`。
fn eligible(dir: &Path, cycle: &str) -> bool {
    let named = dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split_once("_cycle_"))
        .is_some_and(|(_, id)| id == cycle);
    let pending = dir
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|parent| parent == "_pending");
    named
        && !pending
        && dir.is_dir()
        && crate::image_file::checked_source_path(dir).is_ok()
        && matches!(plain_file(&dir.join("part.json")), Ok(true))
        && !dir.join(crate::history_package::PENDING).exists()
}

/// 压缩一件的录制目录：先收拾中断残留，再逐帧压缩；每帧之前要 `external` 说没有业务，并从 `activity` 取到这一帧的许可，
/// 取不到就停下（`interrupted`），下次从剩下的帧继续。
pub(crate) fn compress_part(
    dir: &Path,
    cycle: &str,
    activity: &Activity,
    quiet: Duration,
    external: &dyn Fn() -> bool,
    memo: &mut Memo,
) -> (PartStats, Vec<String>) {
    let mut stats = PartStats::default();
    if !eligible(dir, cycle) {
        return (stats, Vec::new());
    }
    let permit = || {
        if external() {
            activity.touch();
            return None;
        }
        activity.frame(quiet)
    };
    let (warnings, interrupted) = recover(dir, &permit);
    if interrupted {
        stats.interrupted = true;
        return (stats, warnings);
    }
    let frames: Vec<PathBuf> = entries(dir)
        .unwrap_or_default()
        .into_iter()
        .filter(|path| has_extension(path, "pgm") && !memo.kept.contains(path))
        .collect();
    for frame in frames {
        let Some(_permit) = permit() else {
            stats.interrupted = true;
            return (stats, warnings);
        };
        match compress_frame(&frame) {
            FrameOutcome::Compressed { before, after } => {
                stats.frames += 1;
                stats.before += before;
                stats.after += after;
            }
            FrameOutcome::Kept => {
                memo.kept.insert(frame);
            }
            FrameOutcome::Gone => {}
            FrameOutcome::Failed(error) => {
                stats.failed += 1;
                stats
                    .first_error
                    .get_or_insert_with(|| format!("{}：{error}", frame.display()));
                if stats.failed >= PART_FAILURES {
                    return (stats, warnings);
                }
            }
        }
    }
    if stats.failed == 0 {
        memo.done.insert(dir.to_path_buf());
    }
    (stats, warnings)
}

#[cfg(windows)]
fn background_priority() {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }
    // THREAD_MODE_BACKGROUND_BEGIN：CPU、磁盘 IO 与内存优先级一起降到后台
    const THREAD_MODE_BACKGROUND_BEGIN: i32 = 0x0001_0000;
    unsafe {
        SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
    }
}

#[cfg(not(windows))]
fn background_priority() {}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Disabled,
    Paused(&'static str),
    Running,
    Resting,
    Failed,
}

#[derive(Default)]
struct Totals {
    parts: u64,
    frames: u64,
    saved: u64,
}

struct Worker {
    app: AppHandle,
    memo: Memo,
    state: Option<State>,
    /// 上一条状态日志记的状态与时刻
    logged: Option<(State, Instant)>,
    totals: Totals,
    /// 当前这一轮处理到的记录编号
    cursor: i64,
    /// 处理到一半被业务打断的件
    partial: Option<(PathBuf, PartStats)>,
    /// 一轮扫完的时刻与当时的业务活动戳
    rested: Option<(Instant, u64)>,
}

fn megabytes(bytes: u64) -> f64 {
    bytes as f64 / 1_048_576.0
}

/// 每帧之前看的业务状态（只读原子量，不取锁）：有在途工件或未结 PLC 事务、录制队列没写完。
fn in_flight(app: &AppHandle) -> bool {
    app.try_state::<crate::cycle::CycleHost>()
        .is_none_or(|cycle| cycle.busy() || cycle.in_flight() || cycle.recorders.queued() > 0)
}

/// 压缩线程以外的业务状态：有在途工件或未结 PLC 事务、录制队列没写完、有待入库的录制或追溯、有登记的任务。
/// 每秒轮询与每件开始前看（要取追溯与任务登记的锁，只读一下就放）；逐帧只看 [`in_flight`] 与 [`Activity`]。
fn business(app: &AppHandle) -> Option<&'static str> {
    let Some(cycle) = app.try_state::<crate::cycle::CycleHost>() else {
        return Some("检测服务未就绪");
    };
    if cycle.busy() || cycle.in_flight() {
        return Some("有在途工件或未结 PLC 事务");
    }
    if cycle.recorders.queued() > 0 {
        return Some("录制队列未写完");
    }
    let settled = matches!(
        cycle.audit.readiness(),
        Ok(crate::audit::AuditReadiness::Ready)
    ) && cycle
        .audit
        .protected_cycle_ids()
        .is_ok_and(|cycles| cycles.is_empty());
    if !settled {
        return Some("有待入库的录制或追溯");
    }
    if app
        .try_state::<crate::operation::Operations>()
        .is_some_and(|operations| operations.active() > 0)
    {
        return Some("有进行中的任务");
    }
    None
}

impl Worker {
    fn set_state(&mut self, state: State) {
        self.state = Some(state);
        self.log_state();
    }

    /// 状态变了记一条日志；运行与暂停之间来回切换时最少隔 [`STATE_LOG`]，到时再记当时的状态。
    fn log_state(&mut self) {
        let Some(state) = self.state else { return };
        let flapping = |state: State| matches!(state, State::Running | State::Paused(_));
        let skip = match self.logged {
            Some((State::Paused(_), _)) if matches!(state, State::Paused(_)) => true,
            Some((logged, _)) if logged == state => true,
            Some((logged, at)) => flapping(logged) && flapping(state) && at.elapsed() < STATE_LOG,
            None => false,
        };
        if skip {
            return;
        }
        self.logged = Some((state, Instant::now()));
        let total = format!(
            "本次运行累计压缩 {} 件 {} 帧，节省 {:.1} MB",
            self.totals.parts,
            self.totals.frames,
            megabytes(self.totals.saved)
        );
        let (tone, message) = match state {
            State::Disabled => ("info", "已关闭（设置页“空闲时压缩历史原图”）".to_string()),
            State::Paused(reason) => ("info", format!("因业务暂停：{reason}。{total}")),
            State::Running => ("info", "系统空闲，开始压缩历史件原图".to_string()),
            State::Resting => ("info", format!("本轮已扫完。{total}")),
            State::Failed => ("warn", format!("出错，本轮停止，稍后重试。{total}")),
        };
        crate::applog::write(tone, EVENT, &message);
    }

    fn blocked(&self) -> Option<&'static str> {
        let activity = activity();
        if let Some(reason) = business(&self.app) {
            activity.touch();
            return Some(reason);
        }
        (!activity.quiet(QUIET)).then_some("业务停止后连续空闲未满一分钟")
    }

    fn report(&mut self, dir: &Path, stats: &PartStats) {
        if stats.frames > 0 {
            self.totals.parts += 1;
            self.totals.frames += u64::from(stats.frames);
            self.totals.saved += stats.before.saturating_sub(stats.after);
        }
        let name = dir.file_name().unwrap_or_default().to_string_lossy();
        if stats.failed > 0 {
            crate::applog::write(
                "warn",
                EVENT,
                &format!(
                    "{name}：{} 帧压缩失败，原文件保留，首个原因：{}",
                    stats.failed,
                    stats.first_error.as_deref().unwrap_or_default()
                ),
            );
        }
        if stats.frames > 0 {
            crate::applog::write(
                "info",
                EVENT,
                &format!(
                    "{name}：压缩 {} 帧，{:.1} MB → {:.1} MB（{:.1}%）",
                    stats.frames,
                    megabytes(stats.before),
                    megabytes(stats.after),
                    stats.after as f64 * 100.0 / stats.before.max(1) as f64
                ),
            );
        }
    }

    /// 处理这一轮剩下的件，返回接下来的状态。
    fn pass(&mut self) -> State {
        loop {
            let store = self.app.state::<crate::store::Store>();
            let page = match store.compressible_recordings(self.cursor, PAGE) {
                Ok(page) => page,
                Err(error) => {
                    crate::applog::write("warn", EVENT, &format!("读取检测记录失败：{error}"));
                    return State::Failed;
                }
            };
            if page.is_empty() {
                return State::Resting;
            }
            for (id, cycle, dir) in page {
                if self.memo.done.contains(&dir) {
                    self.cursor = id;
                    continue;
                }
                if !ENABLED.load(Ordering::SeqCst) {
                    return State::Disabled;
                }
                if let Some(reason) = self.blocked() {
                    return State::Paused(reason);
                }
                let app = self.app.clone();
                let (stats, warnings) = compress_part(
                    &dir,
                    &cycle,
                    activity(),
                    QUIET,
                    &|| in_flight(&app),
                    &mut self.memo,
                );
                for warning in warnings {
                    crate::applog::write("warn", EVENT, &warning);
                }
                let mut total = match self.partial.take() {
                    Some((partial, total)) if partial == dir => total,
                    _ => PartStats::default(),
                };
                total.merge(stats);
                if total.failed >= PART_FAILURES {
                    self.report(&dir, &total);
                    return State::Failed;
                }
                if total.interrupted {
                    self.partial = Some((dir, total));
                    return State::Paused("业务恢复");
                }
                self.report(&dir, &total);
                self.cursor = id;
            }
        }
    }

    fn run(mut self) {
        loop {
            std::thread::sleep(POLL);
            self.log_state();
            if !ENABLED.load(Ordering::SeqCst) {
                self.set_state(State::Disabled);
                continue;
            }
            if let Some(reason) = self.blocked() {
                self.set_state(State::Paused(reason));
                continue;
            }
            if let Some((at, stamp)) = self.rested {
                if activity().stamp() == stamp && at.elapsed() < REST {
                    continue;
                }
                self.rested = None;
                self.cursor = 0;
            }
            self.set_state(State::Running);
            let next = self.pass();
            if matches!(next, State::Resting | State::Failed) {
                self.rested = Some((Instant::now(), activity().stamp()));
            }
            self.set_state(next);
        }
    }
}

/// 启动压缩线程（后台优先级）。
pub fn start(app: &AppHandle) {
    let app = app.clone();
    let spawned = std::thread::Builder::new()
        .name("raw-compress".into())
        .spawn(move || {
            background_priority();
            Worker {
                app,
                memo: Memo::default(),
                state: None,
                logged: None,
                totals: Totals::default(),
                cursor: 0,
                partial: None,
                rested: None,
            }
            .run()
        });
    if let Err(error) = spawned {
        crate::applog::write("warn", EVENT, &format!("压缩线程启动失败：{error}"));
    }
}

/// 测试用：把目录里的 `.pgm` 都换成 `.gsr`（不看压缩收益，夹具里的小图也换），返回换了几帧。
#[cfg(test)]
pub(crate) fn compress_all(dir: &Path) -> usize {
    let frames: Vec<PathBuf> = entries(dir)
        .unwrap()
        .into_iter()
        .filter(|path| has_extension(path, "pgm"))
        .collect();
    for frame in &frames {
        let encoded = gsr::encode(&std::fs::read(frame).unwrap()).unwrap();
        std::fs::write(frame.with_extension(gsr::EXTENSION), encoded).unwrap();
        std::fs::remove_file(frame).unwrap();
    }
    frames.len()
}

#[cfg(test)]
#[path = "raw_compress/tests.rs"]
mod tests;
