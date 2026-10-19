// SYNTHETIC IPC transport for UI tests only. Rust/SQLite tests enforce the real rules.
import type { Candidate, InboxOverview, SourceDetail, SourceSummary, Transport } from '../../src/ipc';
import { syntheticBackend } from './fake-backend';

export const SOURCE_TEXT = 'Bitte Übungsblatt 4 bis zum 14.11.2026 abgeben.';
export function academicBackend() {
  const base = syntheticBackend();
  const candidate: Candidate = {
    id: 'fixture-candidate', source_id: 'fixture-source', source_subject: 'HM1 Übungsblatt 4',
    source_hash: 'fixture-source-hash', kind: 'task', title: 'Submit HM1 sheet 4', due_date: '2026-11-14',
    date_text: '14.11.2026', date_status: 'explicit', excerpt: SOURCE_TEXT, confidence: .8,
    model: 'synthetic-model', extracted_at: '2026-10-19T12:00:00Z', status: 'pending', payload_hash: 'fixture-payload-hash',
    expires_at: '2099-01-01T00:00:00Z', expired: false, task_id: null, source_state: 'current', conflict: false,
    due_time: null, date_basis: 'written', date_anchor: '2026-10-19T09:00:00Z', date_zone: 'Europe/Berlin', date_note: 'Written in the message',
  };
  const source: SourceDetail = {
    id: 'fixture-source', account_name: 'Synthetic course mailbox', provider: 'synthetic', kind: 'email', label: 'FIXTURE',
    sender: 'lecturer@example.invalid', subject: 'HM1 Übungsblatt 4', body_text: SOURCE_TEXT,
    received_at: '2026-10-19T10:00:00Z', ingested_at: '2026-10-19T11:00:00Z', provider_modified_at: null,
    deleted: false, content_hash: candidate.source_hash, analysis: 'unanalysed', candidates: [],
  };
  const control = {
    empty: false, analysisError: null as string | null, approvalError: null as string | null,
    analysisGate: null as Promise<void> | null, outcome: 'candidates',
    connections: ['hiz_email', 'teams_chat', 'teams_channel'].map((channel) => ({
      channel, accounts: 0, state: 'not_configured', evidence: 'none', last_fetch_at: null,
      last_received_at: null, sources: 0, last_error: null,
    })) as InboxOverview['connections'],
  };
  const overview = (): InboxOverview => ({
    connections: control.connections, live_fetch_available: false, sources_total: control.empty ? 0 : 1,
    unanalysed: !control.empty && source.analysis === 'unanalysed' ? 1 : 0,
    pending_candidates: source.candidates.filter((c) => c.status === 'pending').length,
    attention: source.candidates.filter((c) => c.status === 'pending'),
    source_tasks: base.tasks.map((t) => ({ task_id: t.id as string, title: t.title as string,
      due_date: t.due_date as string | null, status: t.status as string, source_id: source.id, source_subject: source.subject })),
  });
  const transport: Transport = async (cmd, args) => {
    const req = (args?.req ?? {}) as Record<string, unknown>;
    if (!['inbox_overview', 'source_list', 'source_get', 'source_analyze', 'source_analyze_cancel',
      'candidate_approve', 'candidate_reject', 'connections_status'].includes(cmd)) return base.transport(cmd, args);
    base.calls.push({ cmd, args });
    switch (cmd) {
      case 'connections_status': return { accounts: [], polling: { enabled: false, interval_minutes: 30 },
        hiz_host: 'mail.hiz-saarland.de', hiz_port: 993, os_vault_available: true };
      case 'inbox_overview': return structuredClone(overview());
      case 'source_list': {
        const row: SourceSummary = { id: source.id, kind: source.kind, label: source.label, sender: source.sender,
          subject: source.subject, snippet: source.body_text, received_at: source.received_at,
          analysis: source.analysis, pending_candidates: overview().pending_candidates };
        return control.empty ? [] : [structuredClone(row)];
      }
      case 'source_get': return structuredClone(source);
      case 'source_analyze': {
        if (control.analysisGate) await control.analysisGate;
        if (control.analysisError) throw { code: 'Offline', message: control.analysisError };
        source.analysis = control.outcome;
        source.candidates = control.outcome === 'candidates' ? [candidate] : [];
        return structuredClone({ status: control.outcome, created: source.candidates.length,
          already_known: 0, dropped_uncited: 0, truncated_input: false, model: 'synthetic-model',
          detail: '', candidates: source.candidates });
      }
      case 'source_analyze_cancel': return true;
      case 'candidate_approve': {
        if (control.approvalError) throw { code: 'ApprovalInvalid', message: control.approvalError };
        candidate.status = 'approved'; candidate.task_id = 'fixture-task';
        if (!base.tasks.length) base.tasks.push({ id: candidate.task_id, title: req.title ?? candidate.title,
          due_date: req.date_resolved ? req.due_date : candidate.due_date, status: 'todo', revision: 1,
          recurrence: null, due_at_local: null });
        return { task_id: candidate.task_id, replayed: false, candidate: structuredClone(candidate) };
      }
      case 'candidate_reject': candidate.status = 'rejected'; return structuredClone(candidate);
    }
  };
  return { ...base, transport, control, candidate, source };
}
