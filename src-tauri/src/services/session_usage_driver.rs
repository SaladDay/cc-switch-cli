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
//! 快照与权威行一致、Unix inode 未变、尾部指纹吻合 → 续传；截断 / Unix inode
//! 变化 / 指纹失配 → 忽略旧行 offset 全量重扫（去重兜底）；无提示或提示与
//! 权威行不符（首次运行、整库从别的机器 WebDAV 同步进来等）→ 回退到从字节
//! 0 按行 offset 跳过的旧路径。本轮结束后写回新提示。
//!
//! 提示的读与写：**读**由调用方一次性预载整张表（`load_all_sync_resume`，类比
//! `get_all_sync_states`）后逐文件传入 `hint`，驱动内不再逐文件查库——既让 mtime
//! skip 前的身份校验零额外 per-file IO，也把稳态"每变化文件一次查询"收敛为每
//! 周期一次全表查询。**写**由调用方在主库进度提交后调用 [`save_resume_hint`]，
//! 其身份（inode + 尾部指纹）取自 outcome 里扫描时同一 fd 采集的值，而非按 path
//! 二次读取，杜绝"提交后同路径被替换、save 阶段读到异源文件"的错配。
//!
//! mtime skip 与身份校验的顺序：mtime skip 发生在身份校验之前，但跳过前会用那次
//! （本就为 mtime 复查而做的）stat 比对预载提示的 size/inode——保留旧 mtime 的
//! 异源 replace（备份恢复 / rsync -t / 低精度 fs 同 tick）因此不会被永久漏过。跨
//! 机器场景（只同步 cc-switch.db + sidecar、不同步会话文件本身）没有匹配提示或
//! 会话文件由本机 app 写入（mtime 必然前进），走各自既有路径，不受影响。
//!
//! 文件身份校验的边界（诚实化）：Unix inode（`file_identity`）关闭
//! rename-replace 类重写与跨机器同步进来的异源文件——inode 变了即证明这已
//! 不是权威 offset 描述的那个文件。原地 `truncate+rewrite`（inode 不变；会话
//! 日志 app 不会这样重写自己的历史文件）仍靠 ≤64B 尾部指纹（`tail_hash`）
//! 这一 append-only 假设下的启发式识别，**非严格保证**：若文件被整体重写成
//! 更大文件、而旧 offset 前 64 字节恰好逐字节不变，则识别不出。现实触发概率
//! 极低（JSONL 每行独立 JSON、日志 app 只追加不改历史），故不追求跨平台完美
//! 方案。
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
    /// fix 1：`byte_pos` 边界前 ≤64 字节的 FNV-1a 尾部指纹，来自扫描时的
    /// **同一 fd**（非按 path 重读）。与进度同一次读取采集，消除"提交后同路径
    /// 被 rename-replace，save 阶段再按 path 重读到异源文件"造成的
    /// 旧进度 + 新身份错配。
    tail_hash: Option<i64>,
    /// fix 1：被扫描文件的身份标识（Unix inode；其他平台 None），同样取自扫描时
    /// 的 fd（`file.metadata()`），而非保存阶段按 path 二次 stat——保证 sidecar 存的
    /// inode 与 offset/tail_hash 描述的是同一个物理文件。
    file_identity: Option<i64>,
}

/// 基础校验：提示与主库权威行完全一致、且文件未被截断。
///
/// fix 2：提示改为由调用方从预载的内存 map 传入（`load_all_sync_resume` 一次
/// 全表查询，类比 `get_all_sync_states`），本函数只做纯过滤，不再逐文件查库——
/// 既让 skip 前的身份校验零额外 IO，也把稳态下"每变化文件一次 hint 查询"收敛为
/// 每周期一次全表查询。
///
/// 普通提示要求已越过至少一个换行边界（`last_offset > 0` 且 `byte_offset > 0`）。
/// 但"边界=0 + 有待确认尾部"是单行无换行 JSONL 的合法状态：首轮记录
/// `(last_line_offset=0, byte_offset=0, pending_tail)`，权威快照本就是 0，若一律
/// 以 `last_offset <= 0` 把它拒之门外，提示永远无法命中 → 每轮全文件重读且
/// 永远 mtime-1、不收敛。故提示携带 pending_tail（`pending_tail_len > 0`）时放行
/// `0/0` 快照，让尾部收敛路径可达（`decide_resume` 对 `byte_offset == 0` 走空窗口
/// 指纹即可）；无 pending_tail 的普通提示仍走原有的非零校验，语义不放宽。
fn filter_matching_resume_hint(
    hint: Option<SyncResumeHint>,
    last_modified: i64,
    last_offset: i64,
) -> Option<SyncResumeHint> {
    let hint = hint?;
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
    /// 有证据表明这已不是权威 offset 描述的那个文件：截断（size < offset）、
    /// Unix inode 变化（rename-replace 类重写、跨机器同步进来的异源文件），
    /// 或续传边界前的 ≤64B 内容指纹失配（append-only 假设下识破原地重写的
    /// 启发式，非严格保证）：权威行 offset 描述的是旧文件，必须忽略它从头
    /// 全量重扫，各 app 的 request_id 去重保证重扫不会重复入库。
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

    // Unix inode 身份校验（在尾部指纹之前）：提示带 inode、且能取到当前文件
    // inode、且两者不同 → rename-replace 类重写或跨机器同步进来的异源文件，
    // 权威 offset 描述的是旧文件，一票否决全量重扫。提示无 inode（旧提示 /
    // 非 Unix）或取不到当前 inode 时跳过本校验，维持既有 tail_hash 启发式
    // （不放宽也不收紧）。
    let identity_changed = match hint.file_identity {
        Some(expected_identity) => file
            .metadata()
            .ok()
            .and_then(|m| file_identity(&m))
            .is_some_and(|current| current != expected_identity),
        None => false,
    };
    if identity_changed {
        log::debug!(
            "[SYNC-DRIVER] 文件 inode 变化（rename-replace/跨机器异源），全量重扫: {file_path}"
        );
        return ResumeDecision::RescanFromZero;
    }

    // 旧版提示无指纹：既不能证实也不能证伪身份，保守走行 offset 路径
    let Some(expected) = hint.tail_hash else {
        log::debug!("[SYNC-DRIVER] 提示缺尾部指纹，回退行 offset 路径: {file_path}");
        return ResumeDecision::LineSkipFallback;
    };

    // 尾部指纹：读 byte_offset 前的窗口与保存时比对（读毕游标恰在 byte_offset）
    let tail_ok = tail_hash_from_file(file, byte_offset).map(|hash| hash == expected);
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

/// 跨平台文件身份：Unix 返回 inode（`ino() as i64`，位模式保留、允许负值），
/// 其他平台返回 None。inode 变化即证明同路径下已换成不同的物理文件
/// （rename-replace 类重写、跨机器同步进来的异源文件），据此可在续传前一票
/// 否决权威 offset。原地 `truncate+rewrite` 不改 inode，不在本校验覆盖内。
#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> Option<i64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino() as i64)
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> Option<i64> {
    None
}

/// 从已打开的文件句柄读取 `byte_pos` 前 ≤64 字节的尾部窗口指纹。
///
/// fix 1：读的是该 **fd 对应的 inode**，不受"同路径被 rename-replace"影响——
/// 与旧的按 path 重开文件相比，扫描与采集身份始终指向同一个物理文件。
/// 对 append-only 文件而言这段字节此后不再变化，即使采集时文件仍在增长也稳定。
/// 读毕游标恰好停在 `byte_pos`（decide_resume 的续读依赖此后置条件）。
fn tail_hash_from_file(file: &mut fs::File, byte_pos: u64) -> Option<i64> {
    let window = byte_pos.min(TAIL_HASH_WINDOW);
    file.seek(SeekFrom::Start(byte_pos - window)).ok()?;
    let mut tail = vec![0u8; window as usize];
    std::io::Read::read_exact(file, &mut tail).ok()?;
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
#[allow(clippy::too_many_arguments)]
pub(crate) fn scan_jsonl_incremental<S, F>(
    file_path: &Path,
    file_mtime: i64,
    last_modified: i64,
    last_offset: i64,
    hint: Option<SyncResumeHint>,
    sidecar_available: bool,
    init_state: impl FnOnce() -> S,
    mut on_line: F,
) -> Result<Option<JsonlScanOutcome>, AppError>
where
    S: Serialize + DeserializeOwned,
    F: FnMut(&mut S, &str, bool),
{
    let file_path_str = file_path.to_string_lossy();

    // mtime：优先使用 walk 阶段的值，回退到一次 metadata 读取，
    // 保留“元数据不可读即报错”语义。`fresh_meta` 保留决定（潜在）skip 的那次
    // stat，供 fix 2 的 skip 前身份校验复用——不额外多做一次 IO。
    let mut fresh_meta: Option<fs::Metadata> = None;
    let mut file_modified = if file_mtime > 0 {
        file_mtime
    } else {
        let metadata = fs::metadata(file_path)
            .map_err(|e| AppError::Config(format!("无法读取文件元数据: {e}")))?;
        let modified = metadata_modified_nanos(&metadata);
        fresh_meta = Some(metadata);
        modified
    };

    // walk 阶段取得的 mtime 可能在长同步期间过期：文件在 walk 之后、处理之前被
    // 追加时，据陈旧的 walk 值会误判"未变化"而跳过本轮刚追加的内容。将要跳过
    // 前重取一次新鲜 mtime 再判定，关闭 walk→处理之间的窗口。重取失败（文件
    // 可能刚被删除）按原值跳过即可，不报错。
    if file_mtime > 0 && file_modified <= last_modified {
        if let Ok(metadata) = fs::metadata(file_path) {
            file_modified = metadata_modified_nanos(&metadata);
            fresh_meta = Some(metadata);
        }
    }

    // 匹配的续传提示（快照与主库权威行一致）：skip 前身份校验与后续
    // decide_resume 共用。提示由调用方从预载 map 传入（fix 2），此处纯过滤。
    let matched_hint = filter_matching_resume_hint(hint, last_modified, last_offset);

    // 文件未变化则跳过——但 fix 2：先做 skip 前身份校验。
    if file_modified <= last_modified {
        // 保留旧 mtime 的异源 replace（备份恢复 / rsync -t / 低精度 fs 同 tick）会让
        // 本机 mtime 不前进 → 单纯 mtime-skip 会把异源文件永久视作未变化。跳过前用
        // 已读到的 metadata（零额外 IO：这次 stat 本就为 mtime 复查而做）比对匹配
        // 提示的身份：文件比权威 offset 短（截断/异源）或 Unix inode 变化
        // （rename-replace / 跨机器异源）都证明这不是权威 offset 描述的那个文件 →
        // 不 skip，继续扫描（decide_resume 会 RescanFromZero，request_id 去重兜底）。
        // 无匹配提示或无 metadata（读取失败）时无从证伪 → 维持既有 mtime-skip
        // （跨机器只同步 db、不同步会话文件，是记录在案的已知边界）。
        let identity_suspicious = match (matched_hint.as_ref(), fresh_meta.as_ref()) {
            (Some(h), Some(meta)) => {
                let shrunk = h.byte_offset > 0 && (meta.len() as i64) < h.byte_offset;
                let inode_changed = match h.file_identity {
                    Some(expected) => {
                        file_identity(meta).is_some_and(|current| current != expected)
                    }
                    None => false,
                };
                shrunk || inode_changed
            }
            _ => false,
        };
        if !identity_suspicious {
            return Ok(None);
        }
        log::debug!(
            "[SYNC-DRIVER] mtime 未前进但身份可疑（截断/inode 变化），不 skip 继续扫描: {file_path_str}"
        );
    }

    let mut file =
        fs::File::open(file_path).map_err(|e| AppError::Config(format!("无法打开文件: {e}")))?;
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);

    // 提示携带的"待确认尾部"（上轮以不完整尾行结束时记录）。只在 Resume
    // 决策下用于尾部稳定性确认；decide_resume 会消费 hint，先取出这两列。
    let pending_tail =
        matched_hint
            .as_ref()
            .and_then(|h| match (h.pending_tail_len, h.pending_tail_hash) {
                (Some(len), Some(hash)) if len > 0 => Some((len as u64, hash)),
                _ => None,
            });
    // 字节续传决策（指纹校验可能移动过游标，非 Resume 路径必须归零）
    let decision = decide_resume::<S>(&mut file, file_len, matched_hint, &file_path_str);

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
    // 只重读尾部，尾部两轮稳定时收敛为真实 mtime）。sidecar 不可用
    // （`sidecar_available == false`）时没有提示可续，一律 -1 会让末行无换行的文件
    // 每轮从旧进度全量重读、永不收敛，反而更糟。故仅在有 sidecar 时才 -1；无 sidecar
    // 时记录真实 mtime，
    // 回到 pending_tail 机制引入前的旧语义——同一 mtime tick 内补全该行的竞态
    // 窗口不再被关闭，这是 sidecar 不可用时可接受的退化。
    let recorded_modified = if saw_incomplete_tail && sidecar_available {
        log::debug!("[SYNC-DRIVER] 末尾存在不完整行，记录 mtime-1 以便下轮复查: {file_path_str}");
        file_modified - 1
    } else {
        file_modified
    };

    // fix 1：身份（inode + 边界尾部指纹）取自扫描时的同一 fd，而非保存阶段按
    // path 二次读取——即便提交后同路径被 rename-replace，写回 sidecar 的仍是本轮
    // 真正扫描的那个物理文件的身份，不会出现"旧进度 + 新身份"的错配。
    let file = reader.get_mut();
    let file_identity_val = file.metadata().ok().and_then(|m| file_identity(&m));
    let tail_hash_val = tail_hash_from_file(file, committed_byte_pos);

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
        tail_hash: tail_hash_val,
        file_identity: file_identity_val,
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
            // fix 1：收敛进度停在边界，身份亦取边界处——tail_hash 覆盖 byte_offset
            // 前窗口（读毕游标复位到 byte_offset），inode 取自同一 fd，与 outcome 的
            // (line_offset, byte_pos) 严格对应，不写出错配值。
            let tail_hash_val = tail_hash_from_file(file, byte_offset);
            let file_identity_val = file.metadata().ok().and_then(|m| file_identity(&m));
            Some(JsonlScanOutcome {
                line_offset: boundary_line_offset,
                byte_pos: byte_offset,
                file_modified,
                resume_state_json: serde_json::to_string(state).ok(),
                pending_tail_len: None,
                pending_tail_hash: None,
                tail_hash: tail_hash_val,
                file_identity: file_identity_val,
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
        // fix 1：尾部指纹与文件 inode 均来自扫描时的同一 fd（见 outcome 采集处），
        // 不再按 path 重读——消除"提交后同路径被 rename-replace，save 阶段读到异源
        // 文件"造成的 旧进度 + 新身份 错配。收敛 outcome 的 pending_* 为 None：
        // 写回 NULL 清空待确认尾部，下轮不再复查。
        tail_hash: outcome.tail_hash,
        pending_tail_len: outcome.pending_tail_len,
        pending_tail_hash: outcome.pending_tail_hash,
        file_identity: outcome.file_identity,
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
        // fix 2：提示改为预载后传入。测试保持 `Some(&store)` 接口不变——此处从 store
        // 现取该文件的提示（一次 load 等价于生产侧从预载 map 的一次查找）。
        let hint = resume.and_then(|s| s.load_sync_resume(&path.to_string_lossy()).ok().flatten());
        let mut seen = Vec::new();
        let outcome = scan_jsonl_incremental(
            path,
            file_mtime,
            last_modified,
            last_offset,
            hint,
            resume.is_some(),
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

    /// 修复 1：Unix inode 变化（rename-replace 类重写 / 跨机器同步进来的异源
    /// 文件）在 tail_hash 之前触发全量重扫。构造方式：首轮扫描后 save 提示
    /// （记录真实 inode），再把提示的 file_identity 篡改成一个与真实 inode 保证
    /// 不同的值（`^ 1` 翻转最低位）——等价于同路径换了一个 inode。append 新行
    /// 后文件本体 inode 未变、前缀字节不变（tail_hash 本会命中并只读新行），
    /// 但提示 inode 已失配 → inode 校验优先触发全量重扫（所有行 is_new=true）。
    /// 非 Unix 平台 file_identity 恒为 None，本校验不适用，故 gate 掉。
    #[cfg(unix)]
    #[test]
    fn changed_inode_triggers_rescan_from_zero() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\nl2\n").expect("write");
        let store = ScanCacheStore::in_memory().expect("store");

        let first = scan_at(&path, 1_000, 0, 0, Some(&store));
        assert_eq!(first.out().byte_pos, 6);
        save_resume_hint(Some(&store), &path.to_string_lossy(), first.out());

        // 提示已记录真实 inode；篡改成保证不同的值模拟 inode 变化
        let mut hint = store
            .load_sync_resume(&path.to_string_lossy())
            .expect("load")
            .expect("hint");
        let real_inode = hint.file_identity.expect("unix 应记录 inode");
        hint.file_identity = Some(real_inode ^ 1);
        store.save_sync_resume(&hint).expect("save tampered");

        // append-only 追加新行：本体 inode 不变、前缀不变（tail_hash 会命中），
        // 唯有提示的 file_identity 已失配
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"l3\n")
            .unwrap();

        let second = scan_at(
            &path,
            2_000,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        // inode 失配在 tail_hash 之前一票否决 → 全量重扫，所有行 is_new=true
        assert_eq!(
            second.seen,
            vec![
                ("l1".to_string(), true),
                ("l2".to_string(), true),
                ("l3".to_string(), true)
            ]
        );
        assert_eq!(second.out().line_offset, 3);
    }

    /// 修复 1 回归：旧提示 `file_identity == None`（升级前写入 / 非 Unix）时
    /// inode 校验跳过，续传行为与本修复前完全一致——append 新行、前缀字节不变，
    /// tail_hash 命中，只读新行。全平台适用（显式清空 file_identity）。
    #[test]
    fn legacy_hint_without_file_identity_still_resumes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\nl2\n").expect("write");
        let store = ScanCacheStore::in_memory().expect("store");

        let first = scan_at(&path, 1_000, 0, 0, Some(&store));
        save_resume_hint(Some(&store), &path.to_string_lossy(), first.out());

        // 清空 file_identity，模拟升级前的旧提示（或非 Unix 平台）
        let mut hint = store
            .load_sync_resume(&path.to_string_lossy())
            .expect("load")
            .expect("hint");
        hint.file_identity = None;
        store.save_sync_resume(&hint).expect("save legacy");

        // append 新行，前缀字节不变 → tail_hash 命中，续传只读新行
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"l3\n")
            .unwrap();

        let second = scan_at(
            &path,
            2_000,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        assert_eq!(
            second.seen,
            vec![("l3".to_string(), true)],
            "无 file_identity 时应沿用 tail_hash 续传，只读新行"
        );
        assert_eq!(second.out().line_offset, 3);
    }

    /// fix 1：outcome 携带的 file_identity 必须等于被扫描文件的真实 inode（取自
    /// 扫描 fd，而非保存阶段按 path 二次 stat），tail_hash 亦被采集。#[cfg(unix)]。
    #[cfg(unix)]
    #[test]
    fn outcome_carries_scanned_file_real_inode() {
        use std::os::unix::fs::MetadataExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\nl2\n").expect("write");
        let real_inode = std::fs::metadata(&path).expect("metadata").ino() as i64;

        // resume=None 也应采集身份（采集在驱动内无条件进行，与 sidecar 无关）
        let observed = scan_at(&path, 1_000, 0, 0, None);
        assert_eq!(
            observed.out().file_identity,
            Some(real_inode),
            "outcome 的 file_identity 应等于被扫描文件真实 inode"
        );
        assert!(
            observed.out().tail_hash.is_some(),
            "outcome 应采集边界尾部指纹"
        );
    }

    /// fix 2：保留旧 mtime 的异源 rename-replace（备份恢复 / rsync -t / 低精度 fs
    /// 同 tick）。权威 mtime 取一个远大于任何真实文件 mtime 的值，保证替换后的
    /// 真实 mtime 必然 <= 它 → 走 mtime-skip 候选路径，真正考验 skip 前身份校验：
    /// 匹配提示的 inode 与当前文件不符 → 不 skip 继续扫描（decide_resume 走
    /// RescanFromZero），异源文件被完整重扫而非永久漏过。#[cfg(unix)]（inode 校验）。
    #[cfg(unix)]
    #[test]
    fn stale_mtime_replace_with_new_inode_bypasses_skip() {
        // 远大于任何真实文件 mtime（~1.7e18ns）、且 < i64::MAX。
        const BIG_MTIME: i64 = 9_000_000_000_000_000_000;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "l1\nl2\n").expect("write");
        let store = ScanCacheStore::in_memory().expect("store");

        // 首轮：权威进度 (mtime=BIG_MTIME, offset=2, byte=6)，提示记真实 inode。
        let first = scan_at(&path, BIG_MTIME, 0, 0, Some(&store));
        assert_eq!(first.out().byte_pos, 6);
        assert_eq!(first.out().file_modified, BIG_MTIME);
        save_resume_hint(Some(&store), &path.to_string_lossy(), first.out());

        // rename-replace 成异源文件（必然新 inode），内容更长（size >= byte_offset，
        // 只有 inode 校验能识破，size 截断分支不触发），真实 mtime << BIG_MTIME。
        let repl = dir.path().join("repl.jsonl");
        std::fs::write(&repl, "n1\nn2\nn3\n").expect("write repl");
        std::fs::rename(&repl, &path).expect("rename-replace");

        // walk mtime 传 BIG_MTIME（<= 权威）→ 触发新鲜 mtime 复查，读到真实
        // （小）mtime <= BIG_MTIME → skip 候选 → skip 前 inode 校验发现异源。
        let second = scan_at(
            &path,
            BIG_MTIME,
            first.out().file_modified,
            first.out().line_offset,
            Some(&store),
        );
        // 未被 mtime-skip：inode 失配 → RescanFromZero，新文件全部行 is_new。
        assert_eq!(
            second.seen,
            vec![
                ("n1".to_string(), true),
                ("n2".to_string(), true),
                ("n3".to_string(), true)
            ],
            "保留旧 mtime 的异源 replace 不应被 mtime-skip 漏过"
        );
        assert_eq!(second.out().line_offset, 3);
    }
}
