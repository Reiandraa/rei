// SYNTHETIC IPC transport for the automatic-assistant UI tests only. The real queue,
// validation and atomic grouped approval are enforced and tested in Rust/SQLite.
import type { AutoSettings, BatchItem, BatchResult, Candidate, DailySummary, Transport } from '../../src/ipc';
import { syntheticBackend } from './fake-backend';

export function cand(i: number, extra: Partial<Candidate> = {}): Candidate {
  return {
    id: `fixture-cand-${i}`, source_id: `fixture-src-${i}`, source_subject: `Synthetic sheet ${i}`,
    source_hash: `fixture-src-hash-${i}`, kind: 'deadline', title: `Submit sheet ${i}`, due_date: '2026-11-14',
    date_text: '14.11.2026', date_status: 'explicit', excerpt: `Abgabe Blatt ${i} bis 14.11.2026.`, confidence: .8,
    model: 'synthetic-model', extracted_at: '2026-10-19T12:00:00Z', status: 'pending', payload_hash: `fixture-hash-${i}`,
    expires_at: '2099-01-01T00:00:00Z', expired: false, task_id: null, source_state: 'current', conflict: false,
    due_time: null, date_basis: 'written', date_anchor: '2026-10-19T09:00:00Z', date_zone: 'Europe/Berlin', date_note: 'Written in the message', ...extra,
  };
}

export function autoBackend() {
  const base = syntheticBackend();
  const settings: AutoSettings = { enabled: false, paused: false, model: 'qwen3.5:9b', worker_state: 'idle', worker_detail: '', backoff_until: null };
  const control = {
    actionable: [] as Candidate[], updates: [] as Candidate[],
    queue: { queued: 0, running: 0, failed: 0, cancelled: 0 },
    routine: [] as DailySummary['routine'], attention: [] as DailySummary['attention'],
    polling: { enabled: false, interval_minutes: 60 },
    /** Candidate IDs the synthetic backend refuses, to exercise the all-or-nothing UX. */
    refuse: new Map<string, string>(),
    batches: [] as BatchItem[][],
  };
  const summary = (req: Record<string, unknown>): DailySummary => {
    const off = Number(req.actionable_offset ?? 0); const limit = Number(req.limit ?? 25);
    const notAnalysed = control.queue.queued + control.queue.running + control.queue.failed + control.queue.cancelled;
    return {
      settings: { ...settings }, inference_busy: control.queue.running > 0, queue: { ...control.queue },
      sources_total: control.actionable.length + control.updates.length + control.routine.length + notAnalysed,
      not_analysed: notAnalysed, actionable_total: control.actionable.length,
      actionable: structuredClone(control.actionable.slice(off, off + limit)),
      needs_decision: control.actionable.filter((c) => c.date_status === 'ambiguous' || c.conflict).length,
      updates_total: control.updates.length,
      updates: structuredClone(control.updates.slice(Number(req.updates_offset ?? 0), Number(req.updates_offset ?? 0) + limit)),
      routine_total: control.routine.length,
      routine: structuredClone(control.routine.slice(Number(req.routine_offset ?? 0), Number(req.routine_offset ?? 0) + limit)),
      attention_total: control.attention.length, attention: structuredClone(control.attention),
      all_analysed: notAnalysed === 0 && control.attention.every((a) => a.state === 'truncated'), // mirrors Rust summary()
    };
  };
  const transport: Transport = async (cmd, args) => {
    const req = (args?.req ?? {}) as Record<string, unknown>;
    if (!['auto_settings_get', 'auto_settings_set', 'auto_summary', 'auto_queue_cancel', 'auto_queue_retry',
      'candidate_reject', 'candidate_batch_approve', 'connections_status', 'polling_set', 'inbox_overview'].includes(cmd)) return base.transport(cmd, args);
    base.calls.push({ cmd, args });
    switch (cmd) {
      case 'inbox_overview': return { connections: [], live_fetch_available: false, sources_total: 0, unanalysed: 0,
        pending_candidates: 0, attention: [], source_tasks: base.tasks.map((t) => ({ task_id: t.id,
          title: t.title, due_date: t.due_date, status: t.status, source_id: 'fixture-source', source_subject: 'Synthetic source' })) };
      case 'connections_status': return { accounts: [], polling: { ...control.polling }, hiz_host: 'mail.hiz-saarland.de',
        hiz_port: 993, os_vault_available: true };
      case 'polling_set': control.polling = { enabled: req.enabled as boolean, interval_minutes: req.interval_minutes as number };
        return { ...control.polling };
      case 'auto_settings_get': return { ...settings };
      case 'auto_settings_set': Object.assign(settings, { enabled: req.enabled, paused: req.paused, model: req.model }); return { ...settings };
      case 'auto_summary': return summary(req);
      case 'auto_queue_cancel': { const n = control.queue.queued + control.queue.running;
        control.queue.cancelled += n; control.queue.queued = 0; control.queue.running = 0; return n; }
      case 'auto_queue_retry': { const n = control.queue.failed + control.queue.cancelled;
        control.queue.queued += n; control.queue.failed = 0; control.queue.cancelled = 0; control.attention = []; return n; }
      case 'candidate_reject': {
        const c = [...control.updates,...control.actionable].find(c=>c.id===req.id);
        if(!c || c.payload_hash!==req.payload_hash) throw {code:'ApprovalInvalid',message:'This is not the version you reviewed.'};
        c.status='rejected';control.updates=control.updates.filter(x=>x.id!==c.id);control.actionable=control.actionable.filter(x=>x.id!==c.id);return structuredClone(c);
      }
      case 'candidate_batch_approve': {
        const items = req.items as BatchItem[];
        control.batches.push(structuredClone(items));
        const bad = (i: BatchItem) => control.refuse.get(i.id)
          ?? (control.actionable.find((c) => c.id === i.id)?.payload_hash === i.payload_hash ? null : 'This is not the version you reviewed.');
        if (items.some(bad)) {
          return { committed: false, created: 0, replayed: 0, results: items.map((i) => ({ id: i.id, ok: false, task_id: null,
            replayed: false, code: bad(i) ? 'ApprovalInvalid' : 'Held',
            message: bad(i) ?? 'Not created yet because other selected items need attention.' })) } satisfies BatchResult;
        }
        for (const i of items) {
          const c = control.actionable.find((x) => x.id === i.id)!;
          base.tasks.push({ id: `task-${i.id}`, title: i.title ?? c.title, due_date: i.date_resolved ? i.due_date : c.due_date,
            status: 'todo', revision: 1, recurrence: null, due_at_local: null });
        }
        control.actionable = control.actionable.filter((c) => !items.some((i) => i.id === c.id));
        return { committed: true, created: items.length, replayed: 0, results: items.map((i) => ({ id: i.id, ok: true,
          task_id: `task-${i.id}`, replayed: false, code: null, message: null })) } satisfies BatchResult;
      }
    }
  };
  return { ...base, transport, control, settings };
}
