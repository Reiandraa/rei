//! Automatic local academic analysis (docs/AUTO-ACADEMIC-NEXT.md). Opt-in: when enabled,
//! a desktop-app thread queues every live source whose current content hash has no
//! analysis yet, runs one loopback-Ollama inference at a time (shared exclusion with
//! manual analysis, DB lock released during inference), and stores results through the
//! existing strict validator. Nothing becomes a task without the owner's exact approval:
//! the daily summary groups pending proposals and `batch_approve` accepts only the
//! displayed IDs and payload hashes. No OS service; runs only while the app is open.
use crate::commands::{AppState, IpcError};
use crate::inbox::{self, ApproveReq, Candidate, Checked, Infer};
use crate::planner::Ctx;
use crate::{ollama, time};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

type Res<T> = Result<T, IpcError>;

/// Default local model on every platform; the owner may pick another installed model.
pub const DEFAULT_MODEL: &str = "qwen3.5:9b";
/// Per-message attempts (model errors, unusable answers, crashes) before it is shown as failed.
pub const MAX_ATTEMPTS: i64 = 3;
/// Most proposals one grouped approval may contain.
pub const MAX_BATCH: usize = 50;
/// New jobs queued per scan, so a large catch-up is spread over rounds.
pub const SCAN_LIMIT: usize = 200;
/// Jobs processed per worker round before it sleeps and rescans.
pub const JOBS_PER_ROUND: usize = 20;
/// Page size for scanning proposals; not a cap on the reported totals.
const SUMMARY_PAGE: u32 = 500;

/// True only while the automatic worker holds the inference guard, so pause/cancel
/// never stops a manual analysis.
static WORKER_THREAD: Mutex<Option<std::thread::Thread>> = Mutex::new(None);

pub fn wake_worker() {
    if let Ok(t) = WORKER_THREAD.lock() {
        if let Some(t) = t.as_ref() {
            t.unpark();
        }
    }
}

static WORKER_INFERRING: AtomicBool = AtomicBool::new(false);
// An in-flight inference cannot survive a process restart. A process-local epoch
// also detects pause/resume and model A -> B -> A changes before a result returns.
static SETTINGS_GENERATION: AtomicU64 = AtomicU64::new(0);
pub fn settings_epoch() -> u64 {
    SETTINGS_GENERATION.load(Ordering::SeqCst)
}

fn add_minutes(now: &str, minutes: i64) -> Res<String> {
    Ok(inbox::ts(now)?
        .checked_add(jiff::SignedDuration::from_mins(minutes))
        .map_err(|_| IpcError::internal())?
        .to_string())
}

fn short(s: &str) -> String {
    s.chars().take(280).collect()
}

// ---------- settings ----------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AutoSettings {
    pub enabled: bool,
    pub paused: bool,
    pub model: String,
    /// idle | working | offline | model_missing | error
    pub worker_state: String,
    pub worker_detail: String,
    pub backoff_until: Option<String>,
    /// IANA zone used to interpret relative deadline wording.
    pub date_zone: String,
}

pub fn get_settings(conn: &Connection) -> Res<AutoSettings> {
    let row = conn
        .query_row(
            "SELECT enabled, paused, model, worker_state, worker_detail, backoff_until, date_zone FROM auto_analysis_settings WHERE id = 1",
            [],
            |r| {
                Ok(AutoSettings {
                    enabled: r.get::<_, i32>(0)? == 1,
                    paused: r.get::<_, i32>(1)? == 1,
                    model: r.get(2)?,
                    worker_state: r.get(3)?,
                    worker_detail: r.get(4)?,
                    backoff_until: r.get(5)?,
                    date_zone: r.get(6)?,
                })
            },
        )
        .optional()?;
    Ok(row.unwrap_or(AutoSettings {
        enabled: false,
        paused: false,
        model: DEFAULT_MODEL.into(),
        worker_state: "idle".into(),
        worker_detail: String::new(),
        backoff_until: None,
        date_zone: crate::dates::DEFAULT_ZONE.into(),
    }))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoSetReq {
    pub enabled: bool,
    pub paused: bool,
    pub model: String,
    /// Optional IANA zone change; omitted keeps the current zone.
    #[serde(default)]
    pub date_zone: Option<String>,
}

/// Persists the opt-in. Changing anything clears the offline backoff so the next round
/// retries promptly; pausing or disabling cancels the worker's running inference.
pub fn set_settings(conn: &Connection, now: &str, req: &AutoSetReq) -> Res<AutoSettings> {
    inbox::check_model_name(&req.model)?;
    if let Some(z) = &req.date_zone {
        crate::dates::check_zone(z).map_err(inbox::val)?;
    }
    conn.execute(
        "INSERT INTO auto_analysis_settings(id, enabled, paused, model, worker_state, worker_detail, backoff_until, backoff_failures, updated_at)
         VALUES (1, ?1, ?2, ?3, 'idle', '', NULL, 0, ?4)
         ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, paused = excluded.paused, model = excluded.model,
           worker_state = 'idle', worker_detail = '', backoff_until = NULL, backoff_failures = 0, updated_at = excluded.updated_at",
        params![req.enabled as i32, req.paused as i32, req.model, now],
    )?;
    if let Some(z) = &req.date_zone {
        conn.execute(
            "UPDATE auto_analysis_settings SET date_zone = ?1 WHERE id = 1",
            [z],
        )?;
    }
    SETTINGS_GENERATION.fetch_add(1, Ordering::SeqCst);
    signal_worker_cancel();
    wake_worker();
    get_settings(conn)
}

fn signal_worker_cancel() {
    if WORKER_INFERRING.load(Ordering::SeqCst) {
        inbox::cancel_flag().store(true, Ordering::SeqCst);
    }
}

fn set_worker(conn: &Connection, now: &str, state: &str, detail: &str, backoff: bool) -> Res<()> {
    if backoff {
        let failures: i64 = conn
            .query_row(
                "SELECT backoff_failures FROM auto_analysis_settings WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        // 5, 10, 20, 40, then at most 60 minutes between availability checks.
        let wait = (5i64 << failures.min(4)).min(60);
        conn.execute(
            "UPDATE auto_analysis_settings SET worker_state = ?1, worker_detail = ?2, backoff_until = ?3,
               backoff_failures = backoff_failures + 1 WHERE id = 1",
            params![state, short(detail), add_minutes(now, wait)?],
        )?;
    } else {
        conn.execute(
            "UPDATE auto_analysis_settings SET worker_state = ?1, worker_detail = ?2, backoff_until = NULL, backoff_failures = 0 WHERE id = 1",
            params![state, short(detail)],
        )?;
    }
    Ok(())
}

fn active(s: &AutoSettings, now: &str) -> Res<bool> {
    if !s.enabled || s.paused {
        return Ok(false);
    }
    Ok(match &s.backoff_until {
        Some(b) => inbox::ts(b)? <= inbox::ts(now)?,
        None => true,
    })
}

// ---------- durable queue ----------

/// Crash/restart recovery: a job left `running` goes back to the queue unless it has
/// already used every attempt (a message that keeps crashing the app stops retrying).
pub fn recover(conn: &Connection, now: &str) -> Res<u32> {
    let failed = conn.execute(
        "UPDATE analysis_jobs SET status = 'failed', last_error = 'Stopped repeatedly before finishing.', updated_at = ?2
         WHERE status = 'running' AND attempts >= ?1",
        params![MAX_ATTEMPTS, now],
    )?;
    let requeued = conn.execute(
        "UPDATE analysis_jobs SET status = 'queued', next_attempt_at = ?1, updated_at = ?1 WHERE status = 'running'",
        [now],
    )?;
    Ok((failed + requeued) as u32)
}

/// Marks jobs for deleted/changed sources superseded and queues every live source whose
/// current hash has neither an analysis nor a job. Unchanged analysed sources and
/// cancelled/failed versions are never queued again automatically.
pub fn enqueue(conn: &Connection, now: &str) -> Res<u32> {
    let open: Vec<(String, String, String)> = {
        let mut stmt = conn.prepare(
            "SELECT id, source_id, source_hash FROM analysis_jobs WHERE status IN ('queued','failed','cancelled')",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (job, source_id, hash) in open {
        let cur: Option<(Option<String>, String)> = conn
            .query_row(
                "SELECT subject, body_text FROM source_messages WHERE id = ?1 AND deleted_at IS NULL",
                [&source_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let current = matches!(&cur, Some((s, b)) if inbox::source_hash(s.as_deref(), b) == hash);
        if !current {
            conn.execute(
                "UPDATE analysis_jobs SET status = 'superseded', updated_at = ?2 WHERE id = ?1",
                params![job, now],
            )?;
        }
    }
    let mut stmt = conn.prepare(
        "SELECT id, subject, body_text FROM source_messages WHERE deleted_at IS NULL ORDER BY received_at DESC, id",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    let mut added = 0u32;
    for row in rows {
        if added as usize >= SCAN_LIMIT {
            break;
        }
        let (id, subject, body) = row?;
        let hash = inbox::source_hash(subject.as_deref(), &body);
        let state = inbox::analysis_state(conn, &id, &hash)?;
        // invalid_output covers an unusable manual analysis with no job yet; an existing
        // job for this version keeps its own bounded attempts.
        if !matches!(state.as_str(), "unanalysed" | "outdated" | "invalid_output")
            && !inbox::needs_policy_upgrade(conn, &id, &hash)?
        {
            continue;
        }
        // A version superseded earlier (content changed, then changed back) or a finished
        // job whose usable analysis no longer exists is queued again with fresh attempts;
        // failed/cancelled/queued/running versions are left alone.
        added += conn.execute(
            "INSERT INTO analysis_jobs(id, source_id, source_hash, status, attempts, next_attempt_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'queued', 0, ?4, ?4, ?4)
             ON CONFLICT(source_id, source_hash) DO UPDATE SET status = 'queued', attempts = 0,
               next_attempt_at = excluded.next_attempt_at, last_error = '', updated_at = excluded.updated_at
             WHERE analysis_jobs.status IN ('superseded','done')",
            params![crate::planner::new_id(), id, hash, now],
        )? as u32;
    }
    Ok(added)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Job {
    pub id: String,
    pub source_id: String,
    pub source_hash: String,
    pub attempts: i64,
}

/// Takes the next due job (newest message first) when the worker is enabled, not
/// paused and not in an offline backoff. The attempt is counted at claim time.
pub fn claim(conn: &Connection, now: &str) -> Res<Option<Job>> {
    if !active(&get_settings(conn)?, now)? {
        return Ok(None);
    }
    let job: Option<Job> = conn
        .query_row(
            "SELECT j.id, j.source_id, j.source_hash, j.attempts FROM analysis_jobs j
             LEFT JOIN source_messages m ON m.id = j.source_id
             WHERE j.status = 'queued' AND j.next_attempt_at <= ?1
             ORDER BY m.received_at DESC, j.created_at LIMIT 1",
            [now],
            |r| {
                Ok(Job {
                    id: r.get(0)?,
                    source_id: r.get(1)?,
                    source_hash: r.get(2)?,
                    attempts: r.get(3)?,
                })
            },
        )
        .optional()?;
    let Some(mut job) = job else { return Ok(None) };
    conn.execute(
        "UPDATE analysis_jobs SET status = 'running', attempts = attempts + 1, updated_at = ?2 WHERE id = ?1 AND status = 'queued'",
        params![job.id, now],
    )?;
    job.attempts += 1;
    Ok(Some(job))
}

fn finish(conn: &Connection, now: &str, id: &str, status: &str, error: &str) -> Res<()> {
    conn.execute(
        "UPDATE analysis_jobs SET status = ?2, last_error = ?3, updated_at = ?4 WHERE id = ?1",
        params![id, status, short(error), now],
    )?;
    Ok(())
}

/// Back to the queue without using up an attempt (pause, offline, settings change).
fn requeue_uncounted(conn: &Connection, now: &str, id: &str, at: &str) -> Res<()> {
    conn.execute(
        "UPDATE analysis_jobs SET status = 'queued', attempts = max(attempts - 1, 0), next_attempt_at = ?3, updated_at = ?2
         WHERE id = ?1 AND status = 'running'",
        params![id, now, at],
    )?;
    Ok(())
}

/// A counted failure: retry later with backoff, or stop and show it after MAX_ATTEMPTS.
fn retry_or_fail(conn: &Connection, now: &str, job: &Job, error: &str) -> Res<&'static str> {
    if job.attempts >= MAX_ATTEMPTS {
        finish(conn, now, &job.id, "failed", error)?;
        return Ok("failed");
    }
    let at = add_minutes(now, 10 << job.attempts.min(4))?;
    conn.execute(
        "UPDATE analysis_jobs SET status = 'queued', next_attempt_at = ?3, last_error = ?4, updated_at = ?2 WHERE id = ?1",
        params![job.id, now, at, short(error)],
    )?;
    Ok("retry_later")
}

/// What one worker step did; returned for tests and the worker loop.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Analysis stored with this status (candidates | routine | nothing_found).
    Stored(String),
    /// Unusable model answer stored as invalid_output; retried later or failed.
    Invalid(&'static str),
    /// The source changed or was deleted during inference; nothing saved.
    Superseded,
    /// Paused, disabled, model changed or queue cancelled mid-run; nothing saved.
    Interrupted,
    /// Ollama offline or model missing: worker backs off; the attempt is not counted.
    Unavailable,
    /// Other inference error (timeout etc.): retried later or failed.
    Error(&'static str),
    /// A usable analysis of this version already exists (e.g. manual); nothing re-run.
    AlreadyAnalysed,
}

/// Production inference for the stored model; tests pass a labeled fake instead.
pub fn ollama_infer(model: &str) -> Res<Box<dyn Infer>> {
    let client = ollama::OllamaClient::new(inbox::OLLAMA_URL, inbox::OLLAMA_TIMEOUT_S)
        .map_err(inbox::map_ollama)?;
    Ok(Box::new(inbox::OllamaInfer {
        client,
        model: model.into(),
    }))
}

struct WorkerFlag;
impl Drop for WorkerFlag {
    fn drop(&mut self) {
        WORKER_INFERRING.store(false, Ordering::SeqCst);
    }
}

fn db_lock(db: &Mutex<Connection>) -> Res<std::sync::MutexGuard<'_, Connection>> {
    db.lock().map_err(|_| IpcError::internal())
}

/// One queue step. Returns `Ok(None)` when there is nothing to do, the worker is off or
/// another (manual) analysis holds the inference slot. The DB mutex is released while the
/// model runs; job, settings and source version are re-checked before anything is saved.
pub fn run_one(
    db: &Mutex<Connection>,
    device_id: &str,
    now: &dyn Fn() -> String,
    infer_for: &dyn Fn(&str) -> Res<Box<dyn Infer>>,
) -> Res<Option<Outcome>> {
    run_one_with_factory(db, device_id, now, &|_, model| infer_for(model))
}
pub type InferenceFactory<'a> = dyn Fn(&Connection, &str) -> Res<Box<dyn Infer>> + 'a;

pub fn run_one_with_factory(
    db: &Mutex<Connection>,
    device_id: &str,
    now: &dyn Fn() -> String,
    infer_for: &InferenceFactory<'_>,
) -> Res<Option<Outcome>> {
    let Some(_slot) = inbox::try_begin_inference() else {
        return Ok(None);
    };
    let _flag = WorkerFlag;
    let (job, prep, model, generation, infer) = {
        let conn = db_lock(db)?;
        let t = now();
        let Some(job) = claim(&conn, &t)? else {
            return Ok(None);
        };
        let model = get_settings(&conn)?.model;
        // Analysed meanwhile (e.g. manually): never repeat an unchanged usable analysis.
        let state = inbox::analysis_state(&conn, &job.source_id, &job.source_hash)?;
        if matches!(state.as_str(), "candidates" | "routine" | "nothing_found")
            && !inbox::needs_policy_upgrade(&conn, &job.source_id, &job.source_hash)?
        {
            finish(&conn, &t, &job.id, "done", "")?;
            return Ok(Some(Outcome::AlreadyAnalysed));
        }
        match inbox::prepare(&conn, &job.source_id) {
            Ok(p) if p.source_hash == job.source_hash => {
                WORKER_INFERRING.store(true, Ordering::SeqCst);
                let generation = SETTINGS_GENERATION.load(Ordering::SeqCst);
                let infer = infer_for(&conn, &model);
                (job, p, model, generation, infer)
            }
            Ok(_) | Err(_) => {
                finish(&conn, &t, &job.id, "superseded", "")?;
                return Ok(Some(Outcome::Superseded));
            }
        }
    };
    let raw = infer.and_then(|i| i.infer(&prep.messages, inbox::cancel_flag()));

    let mut conn = db_lock(db)?;
    let t = now();
    let still_running: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM analysis_jobs WHERE id = ?1 AND status = 'running')",
        [&job.id],
        |r| r.get(0),
    )?;
    if !still_running {
        return Ok(Some(Outcome::Interrupted));
    }
    let s = get_settings(&conn)?;
    if !s.enabled
        || s.paused
        || s.model != model
        || SETTINGS_GENERATION.load(Ordering::SeqCst) != generation
        || inbox::cancel_flag().load(Ordering::SeqCst)
    {
        requeue_uncounted(&conn, &t, &job.id, &t)?;
        return Ok(Some(Outcome::Interrupted));
    }
    let raw = match raw {
        Ok(r) => r,
        Err(e)
            if e.code == "Offline"
                || (e.code == "Unavailable" && e.message.contains("not installed")) =>
        {
            let state = if e.code == "Offline" {
                "offline"
            } else {
                "model_missing"
            };
            set_worker(&conn, &t, state, &e.message, true)?;
            requeue_uncounted(&conn, &t, &job.id, &t)?;
            return Ok(Some(Outcome::Unavailable));
        }
        Err(e) if e.code == "Cancelled" => {
            requeue_uncounted(&conn, &t, &job.id, &t)?;
            return Ok(Some(Outcome::Interrupted));
        }
        Err(e) => {
            let r = retry_or_fail(&conn, &t, &job, &e.message)?;
            set_worker(&conn, &t, "error", &e.message, false)?;
            return Ok(Some(Outcome::Error(r)));
        }
    };
    let ctx = Ctx { device_id, now: &t };
    match inbox::store(&mut conn, &ctx, &prep, &model, &raw) {
        Ok(res) if res.status == "invalid_output" => {
            let r = retry_or_fail(&conn, &t, &job, "The model answer could not be used.")?;
            set_worker(&conn, &t, "idle", "", false)?;
            Ok(Some(Outcome::Invalid(r)))
        }
        Ok(res) => {
            finish(&conn, &t, &job.id, "done", "")?;
            set_worker(&conn, &t, "idle", "", false)?;
            Ok(Some(Outcome::Stored(res.status)))
        }
        Err(e) if e.code == "Conflict" => {
            finish(&conn, &t, &job.id, "superseded", "")?;
            Ok(Some(Outcome::Superseded))
        }
        Err(e) => {
            // Never leave the job running; a storage error counts as an attempt.
            retry_or_fail(&conn, &t, &job, &e.message)?;
            Err(e)
        }
    }
}

/// Owner action: stop all waiting work (and the running one). Nothing is retried until
/// the owner asks; the messages stay in Inbox.
pub fn cancel_queue(conn: &Connection, now: &str) -> Res<u32> {
    let n = conn.execute(
        "UPDATE analysis_jobs SET status = 'cancelled', updated_at = ?1 WHERE status IN ('queued','running')",
        [now],
    )?;
    signal_worker_cancel();
    Ok(n as u32)
}

/// Owner action: retry failed/cancelled messages (all, or one source) with fresh attempts.
pub fn retry(conn: &Connection, now: &str, source_id: Option<&str>) -> Res<u32> {
    let n = conn.execute(
        "UPDATE analysis_jobs SET status = 'queued', attempts = 0, next_attempt_at = ?1, last_error = '', updated_at = ?1
         WHERE status IN ('failed','cancelled') AND (?2 IS NULL OR source_id = ?2)",
        params![now, source_id],
    )?;
    conn.execute(
        "UPDATE auto_analysis_settings SET backoff_until = NULL, backoff_failures = 0, worker_state = 'idle', worker_detail = '' WHERE id = 1",
        [],
    )?;
    Ok(n as u32)
}

// ---------- daily summary ----------

#[derive(Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct QueueCounts {
    pub queued: u32,
    pub running: u32,
    pub failed: u32,
    pub cancelled: u32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct SourceRef {
    pub source_id: String,
    pub subject: Option<String>,
    pub sender: Option<String>,
    pub label: String,
    pub received_at: String,
    /// routine | invalid_output | failed | cancelled | truncated
    pub state: String,
    pub detail: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct DailySummary {
    pub settings: AutoSettings,
    pub inference_busy: bool,
    pub queue: QueueCounts,
    pub sources_total: u32,
    /// Live sources whose current version has no analysis yet.
    pub not_analysed: u32,
    pub actionable_total: u32,
    pub actionable: Vec<Candidate>,
    /// Actionable proposals that need a date decision (ambiguous or conflicting).
    pub needs_decision: u32,
    pub updates_total: u32,
    pub updates: Vec<Candidate>,
    pub routine_total: u32,
    pub routine: Vec<SourceRef>,
    pub attention_total: u32,
    pub attention: Vec<SourceRef>,
    /// Every live source has a usable analysis of its current version. Not a claim
    /// that nothing important was missed: routine is a fallible model judgment.
    pub all_analysed: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryReq {
    pub limit: Option<u32>,
    pub actionable_offset: Option<u32>,
    pub updates_offset: Option<u32>,
    pub routine_offset: Option<u32>,
}

fn page<T>(v: Vec<T>, offset: Option<u32>, limit: usize) -> Vec<T> {
    v.into_iter()
        .skip(offset.unwrap_or(0) as usize)
        .take(limit)
        .collect()
}

/// Deterministic summary of stored records; no model call.
pub fn summary(conn: &Connection, now: &str, req: &SummaryReq) -> Res<DailySummary> {
    let limit = req.limit.unwrap_or(25).clamp(1, 100) as usize;
    let mut queue = QueueCounts::default();
    {
        let mut stmt = conn.prepare(
            "SELECT status, count(*) FROM analysis_jobs WHERE status IN ('queued','running','failed','cancelled') GROUP BY status",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?;
        for row in rows {
            let (st, n) = row?;
            match st.as_str() {
                "queued" => queue.queued = n,
                "running" => queue.running = n,
                "failed" => queue.failed = n,
                _ => queue.cancelled = n,
            }
        }
    }
    let mut sources_total = 0u32;
    let mut not_analysed = 0u32;
    let mut routine = Vec::new();
    let mut attention = Vec::new();
    {
        let mut stmt = conn.prepare(
            "SELECT id, subject, sender, label, received_at, body_text FROM source_messages
             WHERE deleted_at IS NULL ORDER BY received_at DESC, id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        for row in rows {
            let (id, subject, sender, label, received_at, body) = row?;
            sources_total += 1;
            let hash = inbox::source_hash(subject.as_deref(), &body);
            let state = inbox::analysis_state(conn, &id, &hash)?;
            let job: Option<(String, String)> = conn
                .query_row(
                    "SELECT status, last_error FROM analysis_jobs WHERE source_id = ?1 AND source_hash = ?2",
                    params![id, hash],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let truncated: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM source_analyses WHERE source_id = ?1 AND source_hash = ?2 AND truncated_input = 1)",
                params![id, hash],
                |r| r.get(0),
            )?;
            let mk = |state: &str, detail: &str| SourceRef {
                source_id: id.clone(),
                subject: subject.clone(),
                sender: sender.clone(),
                label: label.clone(),
                received_at: received_at.clone(),
                state: state.into(),
                detail: detail.into(),
            };
            let job_problem = match &job {
                Some((st, err)) if st == "failed" || st == "cancelled" => {
                    Some((st.clone(), err.clone()))
                }
                _ => None,
            };
            match state.as_str() {
                "unanalysed" | "outdated" => {
                    not_analysed += 1;
                    if let Some((st, err)) = job_problem {
                        attention.push(mk(&st, &err));
                    }
                }
                "invalid_output" => {
                    let (st, err) = job_problem.unwrap_or((
                        "invalid_output".into(),
                        "The model answer could not be used.".into(),
                    ));
                    attention.push(mk(
                        if st == "failed" {
                            "failed"
                        } else {
                            "invalid_output"
                        },
                        &err,
                    ));
                }
                "routine" => {
                    routine.push(mk("routine", ""));
                    if truncated {
                        attention.push(mk(
                            "truncated",
                            "Only the first part of this long message was read.",
                        ));
                    }
                }
                _ if truncated => attention.push(mk(
                    "truncated",
                    "Only the first part of this long message was read.",
                )),
                _ => {}
            }
        }
    }
    let mut pending = Vec::new();
    let mut offset = 0u64;
    loop {
        let rows =
            inbox::candidates_page(conn, now, "c.status = ?1", "pending", SUMMARY_PAGE, offset)?;
        let count = rows.len();
        pending.extend(
            rows.into_iter()
                .filter(|c| c.source_state == "current" && !c.expired),
        );
        if count < SUMMARY_PAGE as usize {
            break;
        }
        offset += count as u64;
    }
    let (updates, mut actionable): (Vec<Candidate>, Vec<Candidate>) =
        pending.into_iter().partition(|c| c.kind == "update");
    // Stable order for paging: dated first (soonest), then undated, then ID.
    actionable.sort_by(|a, b| {
        (a.due_date.is_none(), &a.due_date, &a.id).cmp(&(b.due_date.is_none(), &b.due_date, &b.id))
    });
    let needs_decision = actionable
        .iter()
        .filter(|c| c.date_status == "ambiguous" || c.conflict)
        .count() as u32;
    let settings = get_settings(conn)?;
    let all_analysed = not_analysed == 0 && attention.iter().all(|a| a.state == "truncated");
    Ok(DailySummary {
        inference_busy: inbox::inference_busy(),
        queue,
        sources_total,
        not_analysed,
        actionable_total: actionable.len() as u32,
        actionable: page(actionable, req.actionable_offset, limit),
        needs_decision,
        updates_total: updates.len() as u32,
        updates: page(updates, req.updates_offset, limit),
        routine_total: routine.len() as u32,
        routine: page(routine, req.routine_offset, limit),
        attention_total: attention.len() as u32,
        attention: attention.into_iter().take(100).collect(),
        all_analysed,
        settings,
    })
}

// ---------- grouped approval ----------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchReq {
    pub items: Vec<ApproveReq>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BatchItemResult {
    pub id: String,
    pub ok: bool,
    pub task_id: Option<String>,
    pub replayed: bool,
    pub code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct BatchResult {
    /// All-or-nothing: false means no task was created for any item.
    pub committed: bool,
    pub created: u32,
    pub replayed: u32,
    pub results: Vec<BatchItemResult>,
}

/// Approves exactly the listed proposals (ID + payload hash + optional edits) in one
/// transaction. Every item is validated first with the single-approval rules plus the
/// grouped rules; if any item fails, nothing is created and per-item reasons are returned.
pub fn batch_approve(conn: &mut Connection, ctx: &Ctx, req: BatchReq) -> Res<BatchResult> {
    if req.items.is_empty() || req.items.len() > MAX_BATCH {
        return Err(inbox::val(format!("select 1-{MAX_BATCH} items")));
    }
    let mut seen = HashSet::new();
    if !req.items.iter().all(|i| seen.insert(i.id.as_str())) {
        return Err(inbox::val("the same item is listed twice"));
    }
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut checked = Vec::with_capacity(req.items.len());
    let mut stale = Vec::new();
    let mut failed = false;
    for item in &req.items {
        let r = match inbox::check_approval(&tx, ctx, item, true) {
            Ok(Checked::Stale) => {
                stale.push(item.id.clone());
                Err(inbox::stale_error())
            }
            other => other,
        };
        failed |= r.is_err();
        checked.push(r);
    }
    if failed {
        drop(tx);
        if !stale.is_empty() {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            for id in &stale {
                inbox::mark_stale(&tx, ctx, id)?;
            }
            tx.commit()?;
        }
        let results = req
            .items
            .iter()
            .zip(checked)
            .map(|(item, r)| {
                let (code, message) = match r {
                    Err(e) => (e.code, e.message),
                    Ok(_) => (
                        "Held".to_string(),
                        "Not created yet because other selected items need attention.".to_string(),
                    ),
                };
                BatchItemResult {
                    id: item.id.clone(),
                    ok: false,
                    task_id: None,
                    replayed: false,
                    code: Some(code),
                    message: Some(message),
                }
            })
            .collect();
        return Ok(BatchResult {
            committed: false,
            created: 0,
            replayed: 0,
            results,
        });
    }
    let (mut created, mut replayed) = (0, 0);
    let mut results = Vec::with_capacity(req.items.len());
    for (item, r) in req.items.iter().zip(checked) {
        let (task_id, was_replay) = match r? {
            Checked::Replay(t) => (t, true),
            ready => (inbox::execute_approval(&tx, ctx, &item.id, ready)?, false),
        };
        if was_replay {
            replayed += 1;
        } else {
            created += 1;
        }
        results.push(BatchItemResult {
            id: item.id.clone(),
            ok: true,
            task_id: Some(task_id),
            replayed: was_replay,
            code: None,
            message: None,
        });
    }
    tx.commit()?;
    Ok(BatchResult {
        committed: true,
        created,
        replayed,
        results,
    })
}

// ---------- worker thread and Tauri commands ----------

/// Desktop-app thread: recovers interrupted jobs, then rescans and works through the queue
/// in bounded rounds. Idle (one cheap settings read per round) while the opt-in is off.
pub fn start_worker(app: tauri::AppHandle) {
    use tauri::Manager;
    std::thread::spawn(move || {
        if let Ok(mut t) = WORKER_THREAD.lock() {
            *t = Some(std::thread::current());
        }
        let st = app.state::<AppState>();
        if let Ok(c) = st.db.lock() {
            let _ = recover(&c, &time::now_utc_rfc3339());
        }
        loop {
            // Local, model-free date upgrade of older pending results (bounded per round).
            if let Ok(mut c) = st.db.lock() {
                let _ = inbox::upgrade_pending_dates(&mut c, 50);
            }
            let mut did = 0;
            let ready = st
                .db
                .lock()
                .ok()
                .and_then(|c| {
                    let now = time::now_utc_rfc3339();
                    let on = active(&get_settings(&c).ok()?, &now).ok()?;
                    if on {
                        enqueue(&c, &now).ok()?;
                    }
                    Some(on)
                })
                .unwrap_or(false);
            while ready && did < JOBS_PER_ROUND {
                // A panic (e.g. inside the HTTP client) must not end the worker thread for
                // the rest of the session; the interrupted job goes back through `recover`,
                // whose attempt cap stops a message that keeps crashing.
                let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_one_with_factory(
                        &st.db,
                        &st.device_id,
                        &time::now_utc_rfc3339,
                        &|conn, model| {
                            let infer: Box<dyn Infer> = crate::ai::infer_for(conn, model, None)?;
                            Ok(infer)
                        },
                    )
                }));
                match step {
                    Ok(Ok(Some(Outcome::Unavailable))) | Ok(Ok(None)) | Ok(Err(_)) => break,
                    Ok(Ok(Some(_))) => did += 1,
                    Err(_) => {
                        if let Ok(c) = st.db.lock() {
                            let _ = recover(&c, &time::now_utc_rfc3339());
                        }
                        break;
                    }
                }
            }
            let pause = if did > 0 { 2 } else { 20 };
            std::thread::park_timeout(std::time::Duration::from_secs(pause));
        }
    });
}

fn lock(state: &AppState) -> Res<std::sync::MutexGuard<'_, Connection>> {
    db_lock(&state.db)
}

#[tauri::command]
pub fn auto_settings_get(state: tauri::State<'_, AppState>) -> Res<AutoSettings> {
    get_settings(&*lock(&state)?)
}

#[tauri::command]
pub fn auto_settings_set(state: tauri::State<'_, AppState>, req: AutoSetReq) -> Res<AutoSettings> {
    set_settings(&*lock(&state)?, &time::now_utc_rfc3339(), &req)
}

#[tauri::command]
pub fn auto_summary(state: tauri::State<'_, AppState>, req: SummaryReq) -> Res<DailySummary> {
    summary(&*lock(&state)?, &time::now_utc_rfc3339(), &req)
}

#[tauri::command]
pub fn auto_queue_cancel(state: tauri::State<'_, AppState>) -> Res<u32> {
    cancel_queue(&*lock(&state)?, &time::now_utc_rfc3339())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryReq {
    pub source_id: Option<String>,
}

#[tauri::command]
pub fn auto_queue_retry(state: tauri::State<'_, AppState>, req: RetryReq) -> Res<u32> {
    retry(
        &*lock(&state)?,
        &time::now_utc_rfc3339(),
        req.source_id.as_deref(),
    )
}

#[tauri::command]
pub fn candidate_batch_approve(
    state: tauri::State<'_, AppState>,
    req: BatchReq,
) -> Res<BatchResult> {
    let now = time::now_utc_rfc3339();
    let ctx = Ctx {
        device_id: &state.device_id,
        now: &now,
    };
    batch_approve(&mut *lock(&state)?, &ctx, req)
}
