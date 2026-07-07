//! 会话日志增量同步的通用 JSONL 驱动
//!
//! 所有基于 JSONL 会话日志的 app（当前 Claude、Codex，未来新增 app）共用
//! 同一条增量扫描线路，职责划分：
//!
//! - **驱动（本模块）**：mtime 跳过、sidecar 字节续传提示的校验与恢复、
//!   seek 或回退、按字节精确计数的逐行读取、行号/字节位置维护。
//! - **app 适配器（各 session_usage_*.rs）**：一个可 serde 的解析器状态机
//!   `S` + 一个逐行回调（解析行、维护状态、收集待写记录），以及各自
//!   语义的写库阶段（去重规则各 app 不同，刻意不统一）。
//!
//! 进度契约：主库 `session_log_sync` 的 `(last_modified, last_line_offset)`
//! 是权威进度（schema 与上游同步，不可扩展）；sidecar 的
//! `session_sync_resume` 只是加速提示，校验分三档（见 [`ResumeDecision`]）：
//! 快照与权威行一致且尾部指纹吻合 → 续传；截断/指纹失配证明同路径文件被
//! 重写 → 忽略旧行 offset 全量重扫（去重兜底）；无提示或提示与权威行不符
//! （首次运行、整库从别的机器 WebDAV 同步进来等）→ 回退到从字节 0 按行
//! offset 跳过的旧路径。本轮结束后写回新提示。
//!
//! 非 JSONL 数据源（Gemini 整文件 JSON、OpenCode 外部 SQLite）天然无法按
//! 字节续传，仅遵循 mtime 跳过契约，不经过本驱动。

use std::fs;
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::error::AppError;
use crate::services::session_usage::metadata_modified_nanos;
use crate::session_manager::scan_cache_store::{ScanCacheStore, SyncResumeHint};

/// 尾部指纹窗口：`byte_offset` 前至多这么多字节参与 FNV-1a 指纹。
const TAIL_HASH_WINDOW: u64 = 64;

/// FNV-1a 64 位哈希：无依赖、确定性，用作续传边界的内容指纹。
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 一次增量扫描的结果：调用方写库时用 `(file_modified, line_offset)` 更新
/// 主库权威进度，提交成功后用整个 outcome 写回 sidecar 提示。
///
/// `line_offset`/`byte_pos` 只推进到最后一个**换行边界**：文件末尾的不完整
/// 行会进回调（旧行为如此，且写满的最终行可能永远不带换行符），但不计入
/// 持久化进度——正在被追加的半行下个周期从边界重读，去重保证不重复导入，
/// 从而修复了旧行 offset 语义下"半行被计数、补全后被跳过"的永久漏导入。
pub(crate) struct JsonlScanOutcome {
    /// 最后一个换行边界处的行号（与主库 last_line_offset 语义一致）。
    pub line_offset: i64,
    /// 最后一个换行边界处的字节位置。
    pub byte_pos: u64,
    /// 本次使用的文件 mtime 纳秒值。
    pub file_modified: i64,
    /// 换行边界处的状态机序列化快照（不含末尾不完整行的影响）；写进
    /// sidecar 提示，保证续传恢复的状态与恢复的字节位置严格对应。
    resume_state_json: Option<String>,
    /// 本轮结束时"边界之后未终结尾部"的字节数（None = 无待确认尾部）。
    /// 与 `pending_tail_hash` 一起写进 sidecar，供下轮做尾部稳定性确认：
    /// 对永远不带换行的最终行，尾部两轮不变即收敛，不再每周期复查。
    pending_tail_len: Option<i64>,
    /// 上述未终结尾部字节的 FNV-1a 指纹。
    pending_tail_hash: Option<i64>,
}

/// 基础校验：提示与主库权威行完全一致、且文件未被截断。
///
/// 普通提示要求已越过至少一个换行边界（`last_offset > 0` 且 `byte_offset > 0`）。
/// 但"边界=0 + 有待确认尾部"是单行无换行 JSONL 的合法状态：首轮记录
/// `(last_line_offset=0, byte_offset=0, pending_tail)`，权威快照本就是 0，若一律
/// 以 `last_offset <= 0` 把它拒之门外，提示永远无法命中 → 每轮全文件重读且
/// 永远 mtime-1、不收敛。故提示携带 pending_tail（`pending_tail_len > 0`）时放行
/// `0/0` 快照，让尾部收敛路径可达（`decide_resume` 对 `byte_offset == 0` 走空窗口
/// 指纹即可）；无 pending_tail 的普通提示仍走原有的非零校验，语义不放宽。
fn load_matching_resume_hint(
    resume: Option<&ScanCacheStore>,
    file_path: &str,
    last_modified: i64,
    last_offset: i64,
) -> Option<SyncResumeHint> {
    let store = resume?;
    let hint = store.load_sync_resume(file_path).ok().flatten()?;
    // 快照必须与主库权威行完全一致
    if hint.last_modified != last_modified || hint.last_line_offset != last_offset {
        return None;
    }
    let has_pending_tail = matches!(
        (hint.pending_tail_len, hint.pending_tail_hash),
        (Some(len), Some(_)) if len > 0
    );
    if has_pending_tail {
        // 携带待确认尾部：允许 last_offset == 0 与 byte_offset == 0 的单行无换行提示
        Some(hint)
    } else {
        // 普通提示：要求已越过换行边界（首次同步无权威进度时无处可续）
        (last_offset > 0 && hint.byte_offset > 0).then_some(hint)
    }
}

/// 续传决策：区分"能续传"、"文件身份失效需全量重扫"、"沿用行 offset 跳过"。
enum ResumeDecision<S> {
    /// 提示完整有效：从 `byte_offset` 续读，恢复状态机。
    Resume { byte_offset: u64, state: S },
    /// 有确凿证据表明同路径文件已被重写（截断、或续传边界前的内容指纹
    /// 失配）：权威行 offset 描述的是旧文件，必须忽略它从头全量重扫，
    /// 各 app 的 request_id 去重保证重扫不会重复入库。
    RescanFromZero,
    /// 无提示或提示与权威行不符（首次运行、整库从别的机器同步进来、
    /// 提示状态无法反序列化等）：没有身份失效的证据，沿用旧的
    /// 行 offset 跳过路径。
    LineSkipFallback,
}

/// 校验续传提示并做出决策；`Resume` 时文件游标恰好停在 `byte_offset`，
/// 其余情况由调用方负责把游标归零。
fn decide_resume<S: DeserializeOwned>(
    file: &mut fs::File,
    file_len: u64,
    hint: Option<SyncResumeHint>,
    file_path: &str,
) -> ResumeDecision<S> {
    let Some(hint) = hint else {
        // 无提示（首次运行/权威行不匹配）是最常见路径，不值得逐文件记录
        return ResumeDecision::LineSkipFallback;
    };

    let byte_offset = hint.byte_offset as u64;
    // 文件比上次的续传位置还短：同路径被截断/重写过
    if byte_offset > file_len {
        log::debug!(
            "[SYNC-DRIVER] 文件被截断（len={file_len} < offset={byte_offset}），全量重扫: {file_path}"
        );
        return ResumeDecision::RescanFromZero;
    }
    // 旧版提示无指纹：既不能证实也不能证伪身份，保守走行 offset 路径
    let Some(expected) = hint.tail_hash else {
        log::debug!("[SYNC-DRIVER] 提示缺尾部指纹，回退行 offset 路径: {file_path}");
        return ResumeDecision::LineSkipFallback;
    };

    // 尾部指纹：读 byte_offset 前的窗口与保存时比对
    let window = byte_offset.min(TAIL_HASH_WINDOW);
    let tail_ok = (|| {
        file.seek(SeekFrom::Start(byte_offset - window)).ok()?;
        let mut tail = vec![0u8; window as usize];
        std::io::Read::read_exact(file, &mut tail).ok()?;
        Some(fnv1a64(&tail) as i64 == expected)
    })();
    match tail_ok {
        // 指纹失配：边界前内容变了，文件被整体重写
        Some(false) => {
            log::debug!("[SYNC-DRIVER] 尾部指纹失配（同路径被重写），全量重扫: {file_path}");
            return ResumeDecision::RescanFromZero;
        }
        // 读不出来（IO 抖动）：无证据，保守回退
        None => {
            log::debug!("[SYNC-DRIVER] 尾部指纹读取失败，回退行 offset 路径: {file_path}");
            return ResumeDecision::LineSkipFallback;
        }
        Some(true) => {}
    }

    // 指纹吻合但状态机无法恢复（多为本项目升级改了状态结构）：文件身份
    // 没问题，按行 offset 跳过即可
    match hint
        .state
        .as_deref()
        .and_then(|s| serde_json::from_str::<S>(s).ok())
    {
        // read_exact 结束后游标恰好位于 byte_offset，无需再次 seek
        Some(state) => {
            log::debug!("[SYNC-DRIVER] 字节续传命中 offset={byte_offset}: {file_path}");
            ResumeDecision::Resume { byte_offset, state }
        }
        None => {
            log::debug!("[SYNC-DRIVER] 提示状态无法反序列化，回退行 offset 路径: {file_path}");
            ResumeDecision::LineSkipFallback
        }
    }
}

/// 读取文件 `byte_pos` 前的尾部窗口指纹（保存提示时使用）。对 append-only
/// 文件而言这段字节此后不再变化，即使保存时文件仍在增长也稳定。
fn compute_tail_hash(file_path: &str, byte_pos: u64) -> Option<i64> {
    let mut file = fs::File::open(file_path).ok()?;
    let window = byte_pos.min(TAIL_HASH_WINDOW);
    file.seek(SeekFrom::Start(byte_pos - window)).ok()?;
    let mut tail = vec![0u8; window as usize];
    std::io::Read::read_exact(&mut file, &mut tail).ok()?;
    Some(fnv1a64(&tail) as i64)
}

/// 增量扫描单个 JSONL 文件。
///
/// 返回 `Ok(None)` 表示文件自上次同步以来未变化（mtime 跳过）；返回
/// `Ok(Some(outcome))` 表示扫描完成，调用方随后执行自己的写库阶段。
///
/// 回调签名为 `(状态机, 行内容, is_new)`：`is_new == false` 的行只在回退
/// 路径出现（字节续传命中时历史行根本不会被读到），供需要重放历史行来
/// 重建状态的 app（如 Codex 的累计值 delta）使用；无此需求的 app 直接
/// `if !is_new return`。空行与无效 UTF-8 行由驱动跳过，不进回调。
pub(crate) fn scan_jsonl_incremental<S, F>(
    file_path: &Path,
    file_mtime: i64,
    last_modified: i64,
    last_offset: i64,
    resume: Option<&ScanCacheStore>,
    init_state: impl FnOnce() -> S,
    mut on_line: F,
) -> Result<Option<JsonlScanOutcome>, AppError>
where
    S: Serialize + DeserializeOwned,
    F: FnMut(&mut S, &str, bool),
{
    let file_path_str = file_path.to_string_lossy();

    // mtime：优先使用 walk 阶段的值，回退到一次 metadata 读取，
    // 保留“元数据不可读即报错”语义。
    let mut file_modified = if file_mtime > 0 {
        file_mtime
    } else {
        let metadata = fs::metadata(file_path)
            .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
        metadata_modified_nanos(&metadata)
    };

    // walk 阶段取得的 mtime 可能在长同步期间过期：文件在 walk 之后、处理之前被
    // 追加时，据陈旧的 walk 值会误判"未变化"而跳过本轮刚追加的内容。将要跳过
    // 前重取一次新鲜 mtime 再判定，关闭 walk→处理之间的窗口。重取失败（文件
    // 可能刚被删除）按原值跳过即可，不报错。
    if file_mtime > 0 && file_modified <= last_modified {
        if let Ok(metadata) = fs::metadata(file_path) {
            file_modified = metadata_modified_nanos(&metadata);
        }
    }

    // 文件未变化则跳过
    if file_modified <= last_modified {
        return Ok(None);
    }

    let mut file =
        fs::File::open(file_path).map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);

    // 字节续传决策（指纹校验可能移动过游标，非 Resume 路径必须归零）
    let hint = load_matching_resume_hint(resume, &file_path_str, last_modified, last_offset);
    // 提示携带的"待确认尾部"（上轮以不完整尾行结束时记录）。只在 Resume
    // 决策下用于尾部稳定性确认；decide_resume 会消费 hint，先取出这两列。
    let pending_tail =
        hint.as_ref()
            .and_then(|h| match (h.pending_tail_len, h.pending_tail_hash) {
                (Some(len), Some(hash)) if len > 0 => Some((len as u64, hash)),
                _ => None,
            });
    let decision = decide_resume::<S>(&mut file, file_len, hint, &file_path_str);

    // effective_last_offset：本轮用于 is_new 判断的行 offset。文件身份失效
    // 时权威行 offset 描述的是旧文件，必须归零全量重扫，否则新文件的前
    // N 行会被误当作旧行永久漏导入。
    let (mut state, mut line_offset, mut byte_pos, effective_last_offset) = match decision {
        ResumeDecision::Resume { byte_offset, state } => {
            // 上轮以不完整尾行结束、本轮 Resume（边界前指纹已确认稳定）时，
            // 若边界之后的尾部字节也两轮不变 → 收敛：把它当作"永远不带换行
            // 的最终行"，直接返回边界进度 + 真实 mtime + 清空 pending_tail，
            // 之后走正常 mtime skip，不再每周期重读尾部。
            if let Some((tail_len, tail_hash)) = pending_tail {
                if let Some(outcome) = try_converge_stable_tail(
                    &mut file,
                    file_len,
                    byte_offset,
                    last_offset,
                    file_modified,
                    tail_len,
                    tail_hash,
                    &state,
                    &file_path_str,
                ) {
                    return Ok(Some(outcome));
                }
            }
            (state, last_offset, byte_offset, last_offset)
        }
        ResumeDecision::RescanFromZero => {
            file.seek(SeekFrom::Start(0))
                .map_err(|e| AppError::Config(format!("无法定位文件偏移: {e}")))?;
            (init_state(), 0i64, 0u64, 0i64)
        }
        ResumeDecision::LineSkipFallback => {
            file.seek(SeekFrom::Start(0))
                .map_err(|e| AppError::Config(format!("无法定位文件偏移: {e}")))?;
            (init_state(), 0i64, 0u64, last_offset)
        }
    };

    // 持久化进度只推进到换行边界；末尾不完整行进回调但不进进度
    let mut committed_line_offset = line_offset;
    let mut committed_byte_pos = byte_pos;
    let mut resume_state_json: Option<String> = None;
    // 未终结尾部（边界→EOF）的字节数与指纹，遇到不完整尾行时固化，写进
    // sidecar 供下轮做尾部稳定性确认。
    let mut pending_tail_len: Option<i64> = None;
    let mut pending_tail_hash: Option<i64> = None;

    let mut reader = BufReader::new(file);
    let mut raw: Vec<u8> = Vec::new();

    loop {
        raw.clear();
        // read_until 精确返回消耗的字节数（含换行符），字节位置始终可信。
        // 非 EOF 的 IO 错误必须整体报错：若吞掉错误并返回部分结果，调用方
        // 会把本轮 mtime 写入权威进度，错误点之后的行将永久不再导入。
        // 报错让调用方跳过进度更新，下个周期从旧进度完整重试。
        let n = match reader.read_until(b'\n', &mut raw) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                return Err(AppError::Config(format!(
                    "读取会话日志失败 ({file_path_str}): {e}"
                )))
            }
        };
        byte_pos += n as u64;
        line_offset += 1;
        let is_new = line_offset > effective_last_offset;

        if raw.last() == Some(&b'\n') {
            committed_byte_pos = byte_pos;
            committed_line_offset = line_offset;
        } else if resume_state_json.is_none() {
            // 第一次遇到不完整尾行：先固化换行边界处的状态机快照，再让该
            // 行进回调。回调可能据此导入（写满但缺换行的最终行必须导入），
            // 但持久化的 (进度, 状态) 停在边界，下个周期从边界重读该行，
            // 各 app 的 request_id 去重保证不会重复入库。
            resume_state_json = serde_json::to_string(&state).ok();
            // raw 此刻恰好是完整的未终结尾部字节（read_until 遇 EOF 返回、
            // 无换行），直接哈希，无需重开文件再读一遍。
            pending_tail_len = Some(raw.len() as i64);
            pending_tail_hash = Some(fnv1a64(&raw) as i64);
        }

        // 与旧 lines() 语义一致：无效 UTF-8 行跳过
        let Ok(line) = std::str::from_utf8(&raw) else {
            continue;
        };
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }

        on_line(&mut state, line, is_new);
    }

    // 是否见到了不完整尾行（快照在遇到它时已固化）
    let saw_incomplete_tail = resume_state_json.is_some();

    // 无不完整尾行时，边界快照就是最终状态
    if resume_state_json.is_none() {
        resume_state_json = serde_json::to_string(&state).ok();
    }

    // 见到不完整尾行时把记录的 mtime 回退 1ns：若写入方在同一 mtime tick 内
    // 补全了该行且文件此后不再变化，单纯记录当前 mtime 会让下一轮
    // `file_modified <= last_modified` 直接跳过、该行永久漏导。回退 1ns 保证
    // 下一轮必然复查（借助提示从边界 seek，只重读尾部，代价极小）。复查时
    // 若尾部两轮不变（永远无换行的最终行），驱动会走 try_converge_stable_tail
    // 收敛：改记真实 mtime、清空 pending_tail，从此走 mtime skip 不再复查。
    // 关键前提：mtime-1 复查依赖 sidecar 提示才能收敛（下轮借提示从边界 seek、
    // 只重读尾部，尾部两轮稳定时收敛为真实 mtime）。sidecar 不可用（resume 为
    // None）时没有提示可续，一律 -1 会让末行无换行的文件每轮从旧进度全量重读、
    // 永不收敛，反而更糟。故仅在有 sidecar 时才 -1；无 sidecar 时记录真实 mtime，
    // 回到 pending_tail 机制引入前的旧语义——同一 mtime tick 内补全该行的竞态
    // 窗口不再被关闭，这是 sidecar 不可用时可接受的退化。
    let recorded_modified = if saw_incomplete_tail && resume.is_some() {
        log::debug!("[SYNC-DRIVER] 末尾存在不完整行，记录 mtime-1 以便下轮复查: {file_path_str}");
        file_modified - 1
    } else {
        file_modified
    };

    log::debug!(
        "[SYNC-DRIVER] 扫描完成 lines={committed_line_offset} bytes={committed_byte_pos} incomplete_tail={saw_incomplete_tail}: {file_path_str}"
    );

    Ok(Some(JsonlScanOutcome {
        line_offset: committed_line_offset,
        byte_pos: committed_byte_pos,
        file_modified: recorded_modified,
        resume_state_json,
        pending_tail_len,
        pending_tail_hash,
    }))
}

/// 尾部稳定性确认：上轮以不完整尾行结束（sidecar 记录了 pending_tail），
/// 本轮又走 Resume（边界前指纹已确认稳定）时调用。若文件长度恰为
/// `byte_offset + pending_tail_len` 且该尾部字节指纹与上轮吻合，说明两轮
/// 之间没有任何写入——这是"永远不带换行的最终行"，返回收敛 outcome：进度
/// 停在边界、mtime 用真实值（不再 -1）、清空 pending_tail，下轮起走正常
/// mtime skip，不再每周期重读尾部。尾部被补全/继续增长/读取失败时返回
/// None，并把游标复位到 `byte_offset` 供后续续传读取。
#[allow(clippy::too_many_arguments)]
fn try_converge_stable_tail<S: Serialize>(
    file: &mut fs::File,
    file_len: u64,
    byte_offset: u64,
    boundary_line_offset: i64,
    file_modified: i64,
    pending_tail_len: u64,
    pending_tail_hash: i64,
    state: &S,
    file_path_str: &str,
) -> Option<JsonlScanOutcome> {
    // 长度必须精确等于"边界 + 上轮未终结尾部长度"：文件继续增长或被补全
    // （追加了换行 + 新行）都会破坏该等式，交回普通续传处理。
    if file_len != byte_offset + pending_tail_len {
        let _ = file.seek(SeekFrom::Start(byte_offset));
        return None;
    }
    // 读出当前尾部字节比对指纹：等长但内容变了（如尾行被同长度改写）时识破。
    let tail_ok = (|| {
        file.seek(SeekFrom::Start(byte_offset)).ok()?;
        let mut tail = vec![0u8; pending_tail_len as usize];
        std::io::Read::read_exact(file, &mut tail).ok()?;
        Some(fnv1a64(&tail) as i64 == pending_tail_hash)
    })();
    match tail_ok {
        Some(true) => {
            log::debug!("[SYNC-DRIVER] 尾部两轮稳定，收敛（永远无换行的最终行）: {file_path_str}");
            Some(JsonlScanOutcome {
                line_offset: boundary_line_offset,
                byte_pos: byte_offset,
                file_modified,
                resume_state_json: serde_json::to_string(state).ok(),
                pending_tail_len: None,
                pending_tail_hash: None,
            })
        }
        // 尾部变了或读取抖动：复位游标，按普通续传处理（本轮若仍以不完整
        // 行结束会更新 pending_tail）。
        _ => {
            let _ = file.seek(SeekFrom::Start(byte_offset));
            None
        }
    }
}

/// 主库进度提交成功后，把字节位置、状态机快照与尾部指纹写回 sidecar
/// （尽力而为，失败只损失下次的续传加速，不影响正确性）。
pub(crate) fn save_resume_hint(
    resume: Option<&ScanCacheStore>,
    file_path_str: &str,
    outcome: &JsonlScanOutcome,
) {
    let Some(store) = resume else {
        return;
    };
    let hint = SyncResumeHint {
        file_path: file_path_str.to_string(),
        last_modified: outcome.file_modified,
        last_line_offset: outcome.line_offset,
        byte_offset: outcome.byte_pos as i64,
        state: outcome.resume_state_json.clone(),
        tail_hash: compute_tail_hash(file_path_str, outcome.byte_pos),
        // 收敛 outcome 携带 None：写回 NULL 清空待确认尾部；下轮不再复查。
        pending_tail_len: outcome.pending_tail_len,
        pending_tail_hash: outcome.pending_tail_hash,
    };
    if let Err(err) = store.save_sync_resume(&hint) {
        log::debug!("[SESSION-SYNC] 写入字节续传提示失败 ({file_path_str}): {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::io::Write;

    /// 测试用状态机：空壳，仅满足续传往返的 serde 约束；回调观察结果由
    /// 调用方通过外部缓冲捕获（观察记录不是跨轮解析状态，不应进提示）。
    #[derive(Debug, Default, Serialize, Deserialize)]
    struct NoState;

    /// 一次扫描的观察结果：outcome + 回调看到的每一行及其 is_new 标记。
    struct Observed {
        outcome: Option<JsonlScanOutcome>,
        seen: Vec<(String, bool)>,
    }

    impl Observed {
        fn out(&self) -> &JsonlScanOutcome {
            self.outcome.as_ref().expect("changed")
        }
    }

    /// 读取文件当前真实 mtime（纳秒）。fix 3 在"将跳过"时会重取新鲜 mtime 复查，
    /// 因此涉及跳过路径的测试须让 walk mtime 与真实 mtime 一致：写文件后不再改动，
    /// 真实 mtime 恒定，用它作为 walk mtime 即可让复查确认而非误判。
    fn real_mtime(path: &std::path::Path) -> i64 {
        metadata_modified_nanos(&std::fs::metadata(path).expect("metadata"))
    }

    /// `file_mtime` 显式传入（模拟 walk 阶段取得的值）：测试不依赖真实文件
    /// 系统时间戳在两次写入之间前进，避免时间粒度导致的偶发跳过。
    fn scan_at(
        path: &std::path::Path,
        file_mtime: i64,
        last_modified: i64,
        last_offset: i64,
        resume: Option<&ScanCacheStore>,
    ) -> Observed {
        let mut seen = Vec::new();
        let outcome = scan_jsonl_incremental(
            path,
            file_mtime,
            last_modified,
            last_offset,
            resume,
            NoState::default,
            |_state, line, is_new| seen.push((line.to_string(), is_new)),
        )
        .expect("scan");
        Observed { outcome, seen }
    }

    #[test]
    fn first_scan_reads_all_lines_and_reports_positions() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\nl2\n").expect("write");

        let outcome = scan_at(&path, 0, 0, 0, None);
        assert_eq!(
            outcome.seen,
            vec![("l1".to_string(), true), ("l2".to_string(), true)]
        );
        assert_eq!(outcome.out().line_offset, 2);
        assert_eq!(outcome.out().byte_pos, 6);
        assert!(outcome.out().file_modified > 0);
    }

    #[test]
    fn unchanged_file_is_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\n").expect("write");
        // fix 3 会在"将跳过"时重取新鲜 mtime 复查，故 walk mtime 与权威进度都用
        // 文件真实 mtime：复查读到相同的真实 mtime，仍判定未变化 → 跳过。
        let real = real_mtime(&path);
        assert!(scan_at(&path, real, real, 1, None).outcome.is_none());
    }

    #[test]
    fn resume_seeks_past_history_even_when_head_bytes_change() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        // 行足够长，让头部破坏落在尾部指纹窗口（64 字节）之外
        let l1 = "a".repeat(80);
        let l2 = "b".repeat(80);
        std::fs::write(&path, format!("{l1}\n{l2}\n")).expect("write");
        let store = ScanCacheStore::in_memory().expect("store");

        let first = scan_at(&path, 1_000, 0, 0, Some(&store));
        assert_eq!(first.out().byte_pos, 162);
        save_resume_hint(Some(&store), &path.to_string_lossy(), first.out());

        // 破坏头部但保持总字节数不变：把第一个换行符改成空格，两行并作一行。
        // 行式回退路径会因行号偏移而错跳新行；字节续传路径完全不受影响。
        std::fs::write(&path, format!("{l1} {l2}\nl3\n")).expect("rewrite");

        let second = scan_at(
            &path,
            2_000,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        assert_eq!(second.seen, vec![("l3".to_string(), true)]);
        assert_eq!(second.out().line_offset, first.out().line_offset + 1);
        assert_eq!(second.out().byte_pos, 165);
    }

    #[test]
    fn partial_tail_line_does_not_advance_persisted_progress() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        // 末行没有换行符：可能是写到一半，也可能是永远不带换行的最终行
        std::fs::write(&path, "l1\nl2").expect("write");
        let store = ScanCacheStore::in_memory().expect("store");

        let first = scan_at(&path, 1_000, 0, 0, Some(&store));
        // 不完整行仍进回调（写满但缺换行的最终行必须能导入）……
        assert_eq!(
            first.seen,
            vec![("l1".to_string(), true), ("l2".to_string(), true)]
        );
        // ……但持久化进度停在换行边界，且记录的 mtime 回退 1ns，
        // 保证尾行在同一 mtime tick 内补全时下一轮仍会复查
        assert_eq!(first.out().line_offset, 1);
        assert_eq!(first.out().byte_pos, 3);
        assert_eq!(first.out().file_modified, 999);
        save_resume_hint(Some(&store), &path.to_string_lossy(), first.out());

        // 半行被补全并追加新行（append-only，前缀字节不变）
        std::fs::write(&path, "l1\nl2-completed\nl3\n").expect("complete");

        let second = scan_at(
            &path,
            2_000,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        // 从边界续读：补全后的完整行与新行都被处理，没有漏也没有错位
        assert_eq!(
            second.seen,
            vec![("l2-completed".to_string(), true), ("l3".to_string(), true)]
        );
        assert_eq!(second.out().line_offset, 3);
        assert_eq!(second.out().byte_pos, 19);
    }

    /// 稳定的无换行尾行：首轮记 mtime-1 + pending 提示；次轮文件未变、尾部
    /// 两轮稳定 → 返回收敛 outcome（真实 mtime、回调无行）；第三轮 mtime 相等
    /// → Ok(None) 跳过。修复"永远无换行的最终行每周期重读尾部"的不收敛。
    #[test]
    fn stable_unterminated_tail_converges_after_recheck() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        // 末行永远不带换行符（写满的最终行）
        std::fs::write(&path, "l1\nl2").expect("write");
        let store = ScanCacheStore::in_memory().expect("store");
        // 文件写后三轮都不改动：真实 mtime 恒定。fix 3 的复查读到的就是它，故各轮
        // 都用真实 mtime 作为 walk mtime，收敛后的第三轮复查仍判定未变化而跳过。
        let real = real_mtime(&path);

        // 首轮：不完整尾行进回调，进度停边界，mtime 回退 1ns，记录 pending 提示
        let first = scan_at(&path, real, 0, 0, Some(&store));
        assert_eq!(
            first.seen,
            vec![("l1".to_string(), true), ("l2".to_string(), true)]
        );
        assert_eq!(first.out().line_offset, 1);
        assert_eq!(first.out().byte_pos, 3);
        assert_eq!(first.out().file_modified, real - 1);
        save_resume_hint(Some(&store), &path.to_string_lossy(), first.out());

        // sidecar 应记下待确认尾部（"l2" = 2 字节）
        let hint = store
            .load_sync_resume(&path.to_string_lossy())
            .expect("load")
            .expect("hint");
        assert_eq!(hint.pending_tail_len, Some(2));
        assert!(hint.pending_tail_hash.is_some());

        // 次轮：文件未变（传真实 mtime 触发复查：> recorded real-1）。尾部
        // 两轮稳定 → 收敛：回调无行、进度停边界、记录真实 mtime、pending 清空
        let second = scan_at(
            &path,
            real,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        assert!(second.seen.is_empty(), "收敛不应重放任何行");
        assert_eq!(second.out().line_offset, 1);
        assert_eq!(second.out().byte_pos, 3);
        assert_eq!(second.out().file_modified, real, "记录真实 mtime，不再 -1");
        save_resume_hint(Some(&store), &path.to_string_lossy(), second.out());

        let hint = store
            .load_sync_resume(&path.to_string_lossy())
            .expect("load")
            .expect("hint");
        assert_eq!(hint.pending_tail_len, None, "pending 已清空");

        // 第三轮：真实 mtime 相等（fix 3 复查读到相同真实 mtime）→ 正常 mtime skip
        let third = scan_at(
            &path,
            real,
            second.out().file_modified,
            second.out().line_offset,
            Some(&store),
        );
        assert!(third.outcome.is_none(), "收敛后走正常 mtime skip");
    }

    /// fix 1a：单行、永远不带换行的 JSONL + sidecar。首轮 pending 记录
    /// （line_offset=0、byte_pos=0、mtime-1）；次轮 offset=0 的 pending 提示必须被
    /// 接纳（原 `load_matching_resume_hint` 的 `last_offset <= 0` 早退会拒收它，
    /// 导致每轮全文件重读、永远 mtime-1、不收敛）→ 尾部两轮稳定即收敛（真实
    /// mtime、回调零行）；三轮 mtime 相等跳过。
    #[test]
    fn single_line_unterminated_file_converges_with_sidecar() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        // 只有一行，且永远不带换行符：换行边界落在 byte 0
        std::fs::write(&path, "only").expect("write");
        let store = ScanCacheStore::in_memory().expect("store");
        let real = real_mtime(&path);

        // 首轮：不完整尾行进回调，边界停在 0（无任何换行），mtime 回退 1ns
        let first = scan_at(&path, real, 0, 0, Some(&store));
        assert_eq!(first.seen, vec![("only".to_string(), true)]);
        assert_eq!(first.out().line_offset, 0);
        assert_eq!(first.out().byte_pos, 0);
        assert_eq!(first.out().file_modified, real - 1);
        save_resume_hint(Some(&store), &path.to_string_lossy(), first.out());

        // sidecar 记下 offset=0/byte=0 的待确认尾部（"only" = 4 字节）
        let hint = store
            .load_sync_resume(&path.to_string_lossy())
            .expect("load")
            .expect("hint");
        assert_eq!(hint.last_line_offset, 0);
        assert_eq!(hint.byte_offset, 0);
        assert_eq!(hint.pending_tail_len, Some(4));

        // 次轮：offset=0 提示被接纳（修复点），空窗口指纹 + 尾部两轮稳定 → 收敛
        let second = scan_at(
            &path,
            real,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        assert!(second.seen.is_empty(), "收敛不应重放任何行");
        assert_eq!(second.out().line_offset, 0);
        assert_eq!(second.out().byte_pos, 0);
        assert_eq!(second.out().file_modified, real, "记录真实 mtime，不再 -1");
        save_resume_hint(Some(&store), &path.to_string_lossy(), second.out());

        let hint = store
            .load_sync_resume(&path.to_string_lossy())
            .expect("load")
            .expect("hint");
        assert_eq!(hint.pending_tail_len, None, "pending 已清空");

        // 第三轮：mtime 相等 → 正常 mtime skip
        let third = scan_at(
            &path,
            real,
            second.out().file_modified,
            second.out().line_offset,
            Some(&store),
        );
        assert!(third.outcome.is_none(), "收敛后走正常 mtime skip");
    }

    /// fix 1b：末行无换行、resume=None（sidecar 不可用）。无提示可续，一律 -1 会
    /// 让文件每轮从旧进度全量重读、永不收敛，故降级为记录真实 mtime（不 -1），
    /// 下轮 mtime 相等直接跳过。同一 tick 补全该行的竞态窗口作为无 sidecar 的降级
    /// 被接受。
    #[test]
    fn unterminated_tail_without_sidecar_records_real_mtime() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\nl2\nl3").expect("write");
        let real = real_mtime(&path);

        // 首轮：全部行导入；无 sidecar → 记录真实 mtime（不 -1）
        let first = scan_at(&path, real, 0, 0, None);
        assert_eq!(
            first.seen,
            vec![
                ("l1".to_string(), true),
                ("l2".to_string(), true),
                ("l3".to_string(), true)
            ]
        );
        assert_eq!(first.out().line_offset, 2);
        assert_eq!(first.out().byte_pos, 6);
        assert_eq!(
            first.out().file_modified,
            real,
            "无 sidecar 时记录真实 mtime，不 -1"
        );

        // 次轮：mtime 相等（fix 3 复查读到相同真实 mtime）→ 直接跳过
        let second = scan_at(
            &path,
            real,
            first.out().file_modified,
            first.out().line_offset,
            None,
        );
        assert!(second.outcome.is_none());
    }

    /// fix 3：walk 阶段的 mtime 已过期（<= last_modified），但文件真实 mtime 更新。
    /// 驱动应重取新鲜 mtime 后继续扫描，而非据陈旧的 walk 值误跳，关闭
    /// walk→处理之间文件被追加的窗口。
    #[test]
    fn stale_walk_mtime_rechecks_fresh_metadata() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\nl2\n").expect("write");

        // 传入一个过期的 walk mtime（1000 <= last_modified 2000）：据此本应跳过。
        // 但文件真实 mtime 是"刚写入"（远大于 2000），复查后不跳过，读到全部行。
        let observed = scan_at(&path, 1_000, 2_000, 1, None);
        assert_eq!(
            observed.seen,
            vec![("l1".to_string(), false), ("l2".to_string(), true)]
        );
    }

    #[test]
    fn rewritten_larger_file_invalidates_hint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        let l1 = "a".repeat(80);
        std::fs::write(&path, format!("{l1}\n")).expect("write");
        let store = ScanCacheStore::in_memory().expect("store");

        let first = scan_at(&path, 1_000, 0, 0, Some(&store));
        save_resume_hint(Some(&store), &path.to_string_lossy(), first.out());

        // 同路径整体重写成"更大"的文件：size/offset 校验都能通过，
        // 只有尾部指纹能识破 → 必须回退从头扫描
        let rewritten = "z".repeat(200);
        std::fs::write(&path, format!("{rewritten}\n")).expect("rotate");

        let second = scan_at(
            &path,
            2_000,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        // 指纹识破身份失效 → 忽略旧行 offset，全量重扫（is_new=true，
        // 新文件内容不会被误当旧行漏掉；重复导入由 request_id 去重兜底）
        assert_eq!(second.seen, vec![(rewritten, true)]);
        assert_eq!(second.out().line_offset, 1);
    }

    #[test]
    fn mismatched_hint_falls_back_to_line_skip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\nl2\n").expect("write");
        let store = ScanCacheStore::in_memory().expect("store");

        let first = scan_at(&path, 1_000, 0, 0, Some(&store));
        let path_str = path.to_string_lossy().to_string();
        save_resume_hint(Some(&store), &path_str, first.out());

        // 篡改提示的权威快照，模拟主库被外部同步覆盖后的错位
        let mut stale = store
            .load_sync_resume(&path_str)
            .expect("load")
            .expect("hint");
        stale.last_modified += 1;
        store.save_sync_resume(&stale).expect("save");

        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"l3\n")
            .unwrap();

        // 回退路径：历史行以 is_new=false 进回调，新行 is_new=true
        let second = scan_at(
            &path,
            2_000,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        assert_eq!(
            second.seen,
            vec![
                ("l1".to_string(), false),
                ("l2".to_string(), false),
                ("l3".to_string(), true)
            ]
        );
    }

    #[test]
    fn truncated_file_invalidates_hint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "long-line-1\nlong-line-2\n").expect("write");
        let store = ScanCacheStore::in_memory().expect("store");

        let first = scan_at(&path, 1_000, 0, 0, Some(&store));
        let path_str = path.to_string_lossy().to_string();
        save_resume_hint(Some(&store), &path_str, first.out());

        // 文件被截断重写：长度小于提示的字节位置 → 身份失效，全量重扫
        std::fs::write(&path, "x\n").expect("truncate");
        let second = scan_at(
            &path,
            2_000,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        // 新文件的行以 is_new=true 全量重放，不会被旧行 offset 误跳
        assert_eq!(second.seen, vec![("x".to_string(), true)]);
        assert_eq!(second.out().line_offset, 1);
    }
}
