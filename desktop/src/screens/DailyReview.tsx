// Today: automatic local academic assistance (docs/AUTO-ACADEMIC-NEXT.md). The Rust worker
// does the analysis; this view only shows the stored summary and sends the exact displayed
// selection (IDs + payload hashes + owner edits) for one grouped approval.
import { ScheduleNotice, isScheduleNotice } from '../components/ScheduleNotice';
import { ImportantBriefPreview } from './Assistant';
import type { ReactNode } from 'react';
import { InboxFreshness } from '../components/InboxFreshness';
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, asIpcError, sentDay, aiName, isApiModel, modelName, type BatchItem, type BatchResult, type Candidate, type DailySummary, type Polling, type ScheduleApplied } from '../ipc';

const PAGE = 25;

interface Choice { include: boolean; title: string; date: string; time: string }

const needsDecision = (c: Candidate) => c.date_status === 'ambiguous' || c.conflict;
const keyOf = (c: Candidate) => `${c.id}:${c.payload_hash}`;
const time = (iso: string | null) => (iso ? new Date(iso).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '');

function workerLine(d: DailySummary): string {
  const s = d.settings;
  if (s.paused) return 'Paused. Nothing is analysed until you resume.';
  if (s.worker_state === 'offline') return isApiModel(s.model)?`${s.worker_detail || 'API unavailable.'} Retrying around ${time(s.backoff_until)}.`:`Ollama is not running on this computer. Trying again around ${time(s.backoff_until)}.`;
  if (s.worker_state === 'model_missing') return `The model ${s.model} is not installed in Ollama. Install it yourself; nothing is downloaded automatically.`;
  if (d.queue.running > 0 || d.queue.queued > 0) return `${isApiModel(s.model)?`Analysing with ${aiName(s.model)}`:'Analysing locally'}: ${d.queue.running} running, ${d.queue.queued} waiting.`;
  // all_analysed tolerates long messages read only in part; say so instead of an all clear.
  if (d.all_analysed && d.attention_total > 0) return `Every stored message has a result, but ${d.attention_total} long message(s) were only partly read. See Needs attention.`;
  if (d.all_analysed) return d.sources_total===0?'No messages to read yet.':`${d.sources_total===1?'Your stored message has':`All ${d.sources_total} stored messages have`} been analysed.`;
  return `${d.not_analysed} message(s) not analysed yet.`;
}

export function DailyReview({ onTasksChanged, compact = false, configuration = false, extraReview }: { extraReview?:ReactNode; onTasksChanged?: () => Promise<void>; compact?: boolean; configuration?: boolean }) {
  const [d, setD] = useState<DailySummary | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [offset, setOffset] = useState(0);
  const [updatesOffset, setUpdatesOffset] = useState(0);
  const [routineOffset, setRoutineOffset] = useState(0);
  const [choices, setChoices] = useState<Record<string, Choice>>({});
  const [result, setResult] = useState<BatchResult | null>(null);
  const [scheduleApplied, setScheduleApplied] = useState<{result:ScheduleApplied;label:string}|null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [changesOpen,setChangesOpen]=useState(false);
  const [reviewOpen, setReviewOpen] = useState(!compact);
  const [alsoPoll, setAlsoPoll] = useState(true);
  const [polling, setPolling] = useState<Polling | null>(null);
  const offsetRef = useRef(0);
  const updatesOffsetRef = useRef(0);
  const routineOffsetRef = useRef(0);

  const load = useCallback(async (off = offsetRef.current, explicit = false) => {
    try {
      let next = await api.autoSummary(off, PAGE, updatesOffsetRef.current, routineOffsetRef.current);
      const lastPage = (total: number) => Math.max(0, Math.floor((total - 1) / PAGE) * PAGE);
      const actualOff = Math.min(off, lastPage(next.actionable_total));
      const actualUpdates = Math.min(updatesOffsetRef.current, lastPage(next.updates_total));
      const actualRoutine = Math.min(routineOffsetRef.current, lastPage(next.routine_total));
      if (actualOff !== off || actualUpdates !== updatesOffsetRef.current || actualRoutine !== routineOffsetRef.current) {
        next = await api.autoSummary(actualOff, PAGE, actualUpdates, actualRoutine);
      }
      off = actualOff;
      updatesOffsetRef.current = actualUpdates;
      routineOffsetRef.current = actualRoutine;
      offsetRef.current = off;
      setOffset(off);
      setUpdatesOffset(actualUpdates);
      setRoutineOffset(actualRoutine);
      setD(next);
      setErr(null);
      // Keep owner edits/exclusions across refreshes. Items first seen in a background
      // refresh start unticked, so nothing that arrived unseen is approved by accident.
      setChoices((prev) => {
        const out = { ...prev };
        for (const c of next.actionable) {
          out[keyOf(c)] ??= { include: explicit && !needsDecision(c), title: c.title, date: c.due_date ?? '', time: c.due_time ?? '' };
        }
        return out;
      });
    } catch (e) { setErr(asIpcError(e).message); }
  }, []);

  useEffect(() => { void load(0, true); }, [load]);
  useEffect(() => {
    if (!d?.settings.enabled) return;
    const t = setInterval(() => { void load(); }, 15000);
    return () => clearInterval(t);
  }, [d?.settings.enabled, load]);

  const setAssistance = async (enabled: boolean, paused: boolean) => {
    const fresh = await api.autoSettingsGet(); // Keep a model chosen in another Settings section.
    return api.autoSettingsSet(enabled, paused, fresh.model);
  };
  const run = async (f: () => Promise<unknown>) => {
    setBusy(true);
    try { await f(); setErr(null); } catch (e) { setErr(asIpcError(e).message); } finally { setBusy(false); await load(offsetRef.current, true); }
  };

  if (!d) {
    return err ? <section className="card" aria-label="Daily review"><h2>Daily review</h2><p className="muted">{err}</p>{extraReview && <details open={changesOpen} onToggle={e=>setChangesOpen(e.currentTarget.open)}><summary>Other proposed changes</summary>{changesOpen && extraReview}</details>}</section> : null;
  }

  const s = d.settings;
  if (!s.enabled && compact) return <section className="assistant-card" aria-label="Daily review"><span className="overline">Your study assistant</span><h2>Less sorting. More studying.</h2><p>Let your selected AI prepare tasks from your university messages, then review them together.</p><a href="#/settings">Set up automatic assistance →</a>{extraReview && <details open={changesOpen} onToggle={e=>setChangesOpen(e.currentTarget.open)}><summary>Other proposed changes</summary>{changesOpen && extraReview}</details>}</section>;
  if (!s.enabled) {
    return (
      <section className="card" aria-label="Daily review">
        <h2>Automatic study assistant</h2>
        <p>When turned on, REI reads your stored university messages with your selected AI ({modelName(s.model)}),
          including messages already imported and new or changed ones. It prepares suggested tasks and deadlines for one
          daily review here. Nothing becomes a task until you approve it.</p>
        <p className="muted">It works only while this app is open and the computer is awake. {isApiModel(s.model)?`Message text is sent to ${aiName(s.model)} using the API permission saved in Settings.`:'Text stays on this computer with local Ollama.'}</p>
        <label>
          <input type="checkbox" checked={alsoPoll} onChange={(e) => setAlsoPoll(e.target.checked)} />
          {' '}Also check my signed-in, selected sources every 30 minutes
        </label>
        {polling && <p className="muted">Source checking was {polling.enabled ? `on, every ${polling.interval_minutes} minutes` : 'off'}.</p>}
        <div className="actions">
          <button className="primary" disabled={busy} onClick={() => void run(async () => {
            await setAssistance(true, false);
            if (alsoPoll) {
              const before = await api.connectionsStatus().then((c) => c.polling).catch(() => null);
              setPolling(before);
              const after = await api.pollingSet(true, 30);
              setNotice(before && before.enabled && before.interval_minutes === 30
                ? 'Source checking stays every 30 minutes.'
                : `Source checking is now every ${after.interval_minutes} minutes${before ? ` (was ${before.enabled ? `every ${before.interval_minutes} minutes` : 'off'})` : ''}. Source selection is unchanged.`);
            }
          })}>Turn on automatic assistance</button>
        </div>
        {notice && <p role="status">{notice}</p>}
        {err && <p className="muted">{err}</p>}
      </section>
    );
  }

  if (configuration) return <section className="card" aria-label="Automatic assistance"><h2>Automatic assistance</h2><p role="status" aria-label="Assistant status">{workerLine(d)}</p><p className="muted">Uses {aiName(s.model)} while REI is open and this computer is awake. Suggested tasks wait for your approval on Today.</p><div className="actions"><button disabled={busy} onClick={() => void run(() => setAssistance(true, !s.paused))}>{s.paused ? 'Resume' : 'Pause'}</button><button disabled={busy} onClick={() => void run(() => setAssistance(false, false))}>Turn off</button><button disabled={busy} onClick={() => void run(() => api.autoQueueCancel())}>Stop waiting analyses</button><button disabled={busy} onClick={() => void run(() => api.autoQueueRetry(null))}>Retry stopped messages</button></div>{notice && <p role="status">{notice}</p>}{err && <p role="alert">{err}</p>}</section>;

  const onScheduleApplied = async (result:ScheduleApplied,label:string) => {setScheduleApplied({result,label});await onTasksChanged?.();await load(offsetRef.current,true);};
  const shown = d.actionable;
  const selected: BatchItem[] = shown.filter((c) => !isScheduleNotice(c) && choices[keyOf(c)]?.include).map((c) => {
    const ch = choices[keyOf(c)];
    const item: BatchItem = { id: c.id, payload_hash: c.payload_hash };
    if (ch.title.trim() !== c.title) item.title = ch.title;
    // Changing the date keeps the stated time unless the owner changes or clears it too.
    if (needsDecision(c) || ch.date !== (c.due_date ?? '') || ch.time !== (c.due_time ?? '')) {
      item.date_resolved = true;
      item.due_date = ch.date || null;
      if (ch.date && ch.time) item.due_time = ch.time;
    }
    return item;
  });
  const failures = new Map((result && !result.committed ? result.results : []).map((r) => [r.id, r.message ?? r.code ?? '']));
  const set = (c: Candidate, patch: Partial<Choice>) =>
    setChoices((prev) => ({ ...prev, [keyOf(c)]: { ...prev[keyOf(c)], ...patch } }));

  return (
    <section className={compact ? "assistant-card daily-review-compact" : "card"} aria-label="Daily review">
      <div className="daily-review-head"><div><span className="overline">Your study assistant</span><h2>Daily review</h2></div>{compact && <a href="#/settings" aria-label="Assistant settings">Settings ↗</a>}</div>
      {compact && <InboxFreshness showLink={false}/>}
      <p role="status" aria-label="Assistant status">{workerLine(d)}</p>
      {scheduleApplied && <p className="undo-toast" role="status">{scheduleApplied.result.active?`Schedule updated · ${scheduleApplied.label}`:'Schedule change undone.'}{scheduleApplied.result.active && <button disabled={busy} onClick={()=>void run(async()=>{const next=await api.scheduleChangeUndo(scheduleApplied.result.id,scheduleApplied.result.revision);setScheduleApplied({...scheduleApplied,result:next});await onTasksChanged?.();})}>Undo schedule change</button>}</p>}
      {s.worker_detail && s.worker_state === 'error' && <p className="muted">Last problem: {s.worker_detail}</p>}
      {compact && <ImportantBriefPreview/>}
      {extraReview && <details className="other-proposed-changes" open={changesOpen} onToggle={e=>setChangesOpen(e.currentTarget.open)}><summary>Other proposed changes</summary>{changesOpen && extraReview}</details>}
      <p className="review-counts" aria-label="Review counts"><span><strong>{d.actionable_total}</strong> {d.actionable_total===1?'suggestion':'suggestions'}</span><span><strong>{d.updates_total}</strong> {d.updates_total===1?'update':'updates'}</span><span><strong>{d.routine_total}</strong> routine</span>{d.attention_total>0 && <span className="review-attention"><strong>{d.attention_total}</strong> need attention</span>}</p>
      {!compact && <div className="actions">
        <button disabled={busy} onClick={() => void run(() => setAssistance(true, !s.paused))}>
          {s.paused ? 'Resume' : 'Pause'}
        </button>
        {(d.queue.queued > 0 || d.queue.running > 0) && (
          <button disabled={busy} onClick={() => void run(() => api.autoQueueCancel())}>Stop waiting analyses</button>
        )}
        {(d.queue.failed > 0 || d.queue.cancelled > 0) && (
          <button disabled={busy} onClick={() => void run(() => api.autoQueueRetry(null))}>Retry stopped messages</button>
        )}
        <button disabled={busy} onClick={() => void run(() => setAssistance(false, false))}>Turn off</button>
      </div>}
      {compact && !reviewOpen && d.updates.length > 0 && <ul className="important-now" aria-label="Important now">{d.updates.slice(0,3).map(c=><li key={c.id}><div><strong>{c.title}</strong>{isScheduleNotice(c) && <ScheduleNotice candidate={c} onApplied={onScheduleApplied} />}<a href={`#/inbox?source=${c.source_id}`}>{c.source_subject ?? 'Open source'}</a></div><button disabled={busy} aria-label={`Dismiss update: ${c.title}`} onClick={()=>void run(()=>api.candidateReject(c))}>Seen</button></li>)}</ul>}
      <details className="assistant-details" open={reviewOpen} onToggle={e=>setReviewOpen(e.currentTarget.open)}><summary>Review suggestions and updates ({d.actionable_total + d.updates_total + d.attention_total})</summary>
      <h3>Suggested tasks and deadlines</h3>
      {d.actionable_total === 0 ? <p className="muted">No suggestions waiting for review.</p> : (
        <>
          <ul className="rows" aria-label="Suggestions">
            {shown.map((c) => {
              if(isScheduleNotice(c)) return <li key={c.id}><div className="row-main"><strong>{c.title}</strong><a href={`#/inbox?source=${c.source_id}`}>Open source</a><ScheduleNotice candidate={c} onApplied={onScheduleApplied} /></div></li>;
              const ch = choices[keyOf(c)] ?? { include: false, title: c.title, date: c.due_date ?? '', time: c.due_time ?? '' };
              const fail = failures.get(c.id);
              return (
                <li key={c.id} aria-label={`Suggestion: ${c.title}`}>
                  <label>
                    <input type="checkbox" checked={ch.include} onChange={(e) => set(c, { include: e.target.checked })} />
                    {' '}Include
                  </label>
                  <strong>{ch.title}</strong>
                  <details className="suggestion-edit"><summary>Edit suggestion</summary>
                  <input aria-label={`Title for ${c.title}`} value={ch.title} maxLength={200} onChange={(e) => set(c, { title: e.target.value })} />
                  <input aria-label={`Due date for ${c.title}`} type="date" value={ch.date} onChange={(e) => set(c, { date: e.target.value })} />
                  <input aria-label={`Due time for ${c.title}`} type="time" value={ch.time} onChange={(e) => set(c, { time: e.target.value })} />
                  </details>
                  {((c.date_status === 'explicit' && c.due_date) || ch.date) && (
                    <p className="when">Due <strong>{ch.date || 'No due date'}{ch.date && ch.time ? `, ${ch.time}` : ''}</strong>
                      {ch.date !== (c.due_date ?? '') || ch.time !== (c.due_time ?? '') ? ' · edited by you' : c.date_basis === 'relative' ? ` · from “${c.date_text ?? ''}”, sent ${sentDay(c)} (${c.date_zone ?? ''})` : ' · written in the message'}
                    </p>
                  )}
                  {c.date_status === 'ambiguous' && <p className="muted">The message says “{c.date_text}”{c.date_note ? ` (${c.date_note})` : ''}. Enter the date yourself or leave it empty for no date.</p>}
                  {c.conflict && <p className="muted">Another message gives a different date. Check both and enter the correct one.</p>}
                  <blockquote lang="de">{c.excerpt}</blockquote>
                  <p className="muted">From <a href={`#/inbox?source=${c.source_id}`}>{c.source_subject ?? 'message'}</a> · {ch.date && ch.time ? `deadline time ${ch.time} ${c.date_zone ?? ''}` : 'date-only task, no time'}</p>
                  {fail && <p role="alert">{fail}</p>}
                </li>
              );
            })}
          </ul>
          <div className="actions">
            <button disabled={offset === 0 || busy} onClick={() => void load(Math.max(0, offset - PAGE), true)}>Previous</button>
            <span className="muted">{offset + 1}–{offset + shown.length} of {d.actionable_total}</span>
            <button disabled={offset + shown.length >= d.actionable_total || busy} onClick={() => void load(offset + PAGE, true)}>Next</button>
          </div>
          <button className="primary" disabled={busy || selected.length === 0} onClick={() => void run(async () => {
            setResult(null);
            const r = await api.batchApprove(selected);
            setResult(r);
            if (r.committed) await onTasksChanged?.();
            setNotice(r.committed
              ? `Created ${r.created} task(s)${r.replayed ? `; ${r.replayed} already existed` : ''}.`
              : 'Nothing was created. Fix or exclude the marked items and approve again.');
          })}>Approve {selected.length} selected</button>
          <p className="muted">Only the suggestions shown on this page and ticked are approved. New messages arriving later are never included.</p>
        </>
      )}
      {notice && <p role="status">{notice}</p>}
      {err && <p role="alert">{err}</p>}

      {d.updates_total > 0 && (
        <>
          <h3>Important updates</h3>
          <ul className="rows" aria-label="Updates">
            {d.updates.map((c) => (
              <li key={c.id}><div className="row-main">{c.title}{isScheduleNotice(c) && <ScheduleNotice candidate={c} onApplied={onScheduleApplied} />}</div> — <a href={`#/inbox?source=${c.source_id}`}>{c.source_subject ?? 'message'}</a><button disabled={busy} aria-label={`Dismiss update: ${c.title}`} onClick={()=>void run(()=>api.candidateReject(c))}>Seen</button></li>
            ))}
          </ul>
          {d.updates_total > PAGE && <div className="actions" aria-label="Update pages">
            <button disabled={updatesOffset === 0 || busy} onClick={() => { updatesOffsetRef.current = Math.max(0, updatesOffset - PAGE); void load(); }}>Previous updates</button>
            <span className="muted">{updatesOffset + 1}–{updatesOffset + d.updates.length} of {d.updates_total} updates</span>
            <button disabled={updatesOffset + d.updates.length >= d.updates_total || busy} onClick={() => { updatesOffsetRef.current = updatesOffset + PAGE; void load(); }}>Next updates</button>
          </div>}
        </>
      )}
      {d.attention_total > 0 && (
        <>
          <h3>Needs attention</h3>
          <ul className="rows" aria-label="Needs attention">
            {d.attention.map((a) => (
              <li key={`${a.source_id}:${a.state}`}>
                <a href={`#/inbox?source=${a.source_id}`}>{a.subject ?? 'message'}</a> · {{
                  failed: 'analysis stopped after repeated problems', invalid_output: 'the model answer could not be used',
                  cancelled: 'analysis was stopped', truncated: 'only the first part was read', routine: 'routine',
                }[a.state]}{a.detail ? ` (${a.detail})` : ''}
              </li>
            ))}
          </ul>
        </>
      )}
      {d.routine_total > 0 && (
        <details>
          <summary>Filed as routine ({d.routine_total})</summary>
          <p className="muted">Routine is the selected model’s judgment and can be wrong. Nothing was deleted or marked read; every original stays in Inbox.</p>
          <ul className="rows">
            {d.routine.map((r) => <li key={r.source_id}><a href={`#/inbox?source=${r.source_id}`}>{r.subject ?? 'message'}</a></li>)}
          </ul>
          {d.routine_total > PAGE && <div className="actions" aria-label="Routine pages">
            <button disabled={routineOffset === 0 || busy} onClick={() => { routineOffsetRef.current = Math.max(0, routineOffset - PAGE); void load(); }}>Previous routine</button>
            <span className="muted">{routineOffset + 1}–{routineOffset + d.routine.length} of {d.routine_total} routine messages</span>
            <button disabled={routineOffset + d.routine.length >= d.routine_total || busy} onClick={() => { routineOffsetRef.current = routineOffset + PAGE; void load(); }}>Next routine</button>
          </div>}
        </details>
      )}
      </details>
    </section>
  );
}
