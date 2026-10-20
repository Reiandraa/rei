// Typed IPC client. The renderer never talks to providers or holds tokens (ARCHITECTURE.md).
// COMMANDS must equal the keys of schemas/ipc-v1.json (checked by scripts/tools/check-ipc-schema.mjs).
export const COMMANDS = [
  'app_status',
  'ai_settings',
  'local_ai_setup',
  'email_discover',
  'email_configure',
  'email_sign_in',
  'assistant_summary_get',
  'assistant_summary_refresh',
  'assistant_memory',
  'health',
  'settings_get',
  'settings_set',
  'task_list',
  'task_get',
  'task_create',
  'task_update',
  'task_complete',
  'task_delete',
  'occurrence_edit',
  'schedule_change_apply',
  'schedule_change_undo',
  'agenda_list',
  'calendar_day_labels',
  'event_list',
  'event_create',
  'event_update',
  'event_delete',
  'proposal_list',
  'proposal_approve',
  'proposal_reject',
  'planner_suggest_reschedules',
  'inbox_overview',
  'source_list',
  'source_get',
  'source_import_manual',
  'model_status',
  'source_analyze',
  'source_analyze_cancel',
  'candidate_approve',
  'candidate_reject',
  'assistant_workflow_set',
  'connections_status',
  'hiz_configure',
  'hiz_sign_in',
  'teams_configure',
  'teams_sign_in',
  'teams_discover',
  'connection_select_sources',
  'connection_fetch_now',
  'connection_fetch_cancel',
  'connection_disconnect',
  'polling_set',
  'auto_settings_get',
  'auto_settings_set',
  'auto_summary',
  'auto_queue_cancel',
  'auto_queue_retry',
  'candidate_batch_approve',
  'task_plan',
  'task_unplan',
  'task_reopen',
  'task_undo',
  'model_list',
] as const;
export type CommandName = (typeof COMMANDS)[number];

export type IpcErrorCode =
  | 'Validation' | 'NotFound' | 'Conflict' | 'Forbidden' | 'Locked' | 'ApprovalRequired'
  | 'ApprovalInvalid' | 'ProviderAuthRequired' | 'ProviderDenied' | 'RateLimited'
  | 'Offline' | 'Unavailable' | 'Cancelled' | 'Internal';

export interface IpcError { code: IpcErrorCode; message: string; current_revision?: number }

export interface BriefPoint {text:string;excerpt:string}
export interface BriefEntry {source_id:string;subject:string;received_at:string;importance:'important'|'routine'|'uncertain'|'failed';points:BriefPoint[];partial:boolean;detail:string;updated_at:string;priority?:{rank:number;reason:string;due_date:string|null}}
export interface AssistantBrief {state:string;enabled:boolean;model:string;updated_at:string|null;detail:string;total:number;scope_limit:number;pending:number;routine:number;entries:BriefEntry[]}
export interface AssistantNote {id:string;text:string;hash:string}
export type AiProvider='local'|'openai'|'anthropic';
export interface AiSettings {revision:number;provider:AiProvider;model:string;local_model:string;openai_saved:boolean;anthropic_saved:boolean;daily_limit:number;used_today:number;available_models:string[]}
export interface AiRequest {action:'get'|'save_key'|'test'|'use_api'|'use_local'|'delete_key';provider?:Exclude<AiProvider,'local'>;model?:string;api_key?:string;consent?:boolean;daily_limit?:number;expected_revision?:number}
export const isApiModel=(model:string)=>/^api_(openai|anthropic)\//.test(model);
export const aiName=(model:string)=>model.startsWith('api_openai/')?'OpenAI API':model.startsWith('api_anthropic/')?'Claude API':'Local Ollama';
export const modelName=(model:string)=>model.replace(/^api_(openai|anthropic)\//,'');

export interface EmailProfile {profile_id:string;kind:'hiz'|'imap'|'google'|'microsoft';email:string;display_name:string;username:string;server:string|null;publisher_ready:boolean;detail:string}

export interface LocalSetup {platform:string;installed:boolean;memory_gb:number|null;recommended_model:string;download:{state:string;model:string;detail:string;completed:number;total:number}}

export interface AppStatus { ipc_version: number; schema_version: number; lock_state: string; device_id: string }
export interface Health { database_ok: boolean; schema_version: number }
export interface Setting { key: string; value: unknown; revision: number }
export type SettingKey = 'locale' | 'theme' | 'accent' | 'text_size' | 'motion';

export type DueIn = { kind: 'date'; date: string } | { kind: 'at'; local: string; tz: string };
export interface RecurrenceIn { rrule: string; tz: string; dtstart_local: string; weekday_times?: Record<string, string> }
export interface Task {
  id: string; revision: number; title: string; notes: string; status: 'todo' | 'doing' | 'done' | 'cancelled';
  priority: number; energy: string | null; duration_min: number | null;
  due_date: string | null; due_at_utc: string | null; due_tz: string | null; due_at_local: string | null;
  recurrence: (RecurrenceIn & { series_id: string }) | null; reminder_offsets_min: number[];
  created_at: string; updated_at: string;
  /** Work plan (when you intend to work on it) — never the deadline. */
  plan_date: string | null; plan_time: string | null; plan_duration_min: number | null; plan_tz: string | null;
  /** Undo offered for the latest change at this revision. */
  undo_action: 'complete' | 'delete' | 'plan' | 'unplan' | null;
  has_schedule_changes?: boolean; schedule_from?: string | null;
}
export interface TaskCreate {
  title: string; notes?: string; priority?: number; energy?: string; duration_min?: number;
  due?: DueIn; recurrence?: RecurrenceIn; reminder_offsets_min?: number[];
}
export interface OccurrenceEdit {
  task_id: string; expected_revision: number; scope: 'this' | 'following' | 'series';
  action: 'done' | 'skip' | 'move' | 'reset'; original_local: string; new_local?: string;
}
export interface AgendaItem {
  kind: 'task' | 'plan' | 'occurrence' | 'event'; id: string; title: string; local_start: string; all_day: boolean;
  state: string; original_local: string | null; task_revision: number | null; recurring: boolean;
  read_only: boolean; conflict: boolean; location?: string | null;
}
export interface ScheduleApplied { id: string; task_id: string; revision: number; replayed: boolean; active: boolean }
export interface ScheduleApply { candidate_id: string; payload_hash: string; task_id: string; expected_revision: number; scope: 'this' | 'following'; date: string; new_time: string | null; location: string | null }
export interface CalEvent {
  id: string; revision: number; title: string; all_day: boolean; start_local: string; end_local: string;
  start_utc: string | null; end_utc: string | null; tz: string; provider: string | null; read_only: boolean;
}
export interface EventCreate { title: string; all_day: boolean; start_local: string; end_local: string; tz: string }
export interface Proposal {
  id: string; action: string; target_id: string | null; target_title: string | null;
  expected_target_revision: number | null; payload: { due?: DueIn }; payload_hash: string; expires_at: string;
  status: 'Pending' | 'Approved' | 'Rejected' | 'Expired' | 'Executed' | 'Failed';
  confidence: number; rationale: string; created_at: string;
}

export interface Connection { channel: 'hiz_email' | 'teams_chat' | 'teams_channel' | 'manual'; accounts: number; state: string; evidence: string;
  last_fetch_at: string | null; last_received_at: string | null; sources: number; last_error: string | null }
export interface Candidate { id: string; source_id: string; source_subject: string | null; source_hash: string; kind: 'task' | 'deadline' | 'update';
  title: string; due_date: string | null; date_text: string | null; date_status: 'none' | 'explicit' | 'ambiguous'; excerpt: string;
  due_time: string | null; date_basis: 'written' | 'relative' | null; date_anchor: string | null; date_zone: string | null; date_note: string;
  confidence: number; model: string; extracted_at: string; status: 'pending' | 'approved' | 'rejected' | 'stale'; payload_hash: string;
  expires_at: string; expired: boolean; task_id: string | null; source_state: 'current' | 'changed' | 'deleted'; conflict: boolean; schedule_active?: boolean | null }
export interface SourceTask { task_id: string; title: string; due_date: string | null; status: string; source_id: string; source_subject: string | null }
export interface InboxOverview { connections: Connection[]; live_fetch_available: boolean; sources_total: number; unanalysed: number;
  pending_candidates: number; attention: Candidate[]; source_tasks: SourceTask[] }
export interface SourceSummary { id: string; kind: string; label: string; sender: string | null; subject: string | null; snippet: string;
  received_at: string; analysis: string; pending_candidates: number }
export interface SourceDetail { id: string; account_name: string; provider: string; kind: string; label: string; sender: string | null;
  subject: string | null; body_text: string; received_at: string; ingested_at: string; provider_modified_at: string | null; deleted: boolean;
  content_hash: string; analysis: string; candidates: Candidate[] }
export interface AnalysisResult { status: string; created: number; already_known: number; dropped_uncited: number; truncated_input: boolean;
  model: string; detail: string; candidates: Candidate[] }
export interface ApproveEdits { title?: string; due_date?: string | null; due_time?: string | null; date_resolved?: boolean }

export interface ProviderSource { source_kind: 'folder' | 'chat' | 'channel'; source_ref: string; display_name: string; selected: boolean }
export interface ProviderAccount { account_id: string; provider: 'hiz_imap' | 'imap_email' | 'google_mail' | 'microsoft_mail' | 'teams_graph'; kind: string; display_name: string; state: string;
  evidence: 'none' | 'fixture' | 'live'; settings_saved: boolean; sign_in_saved: boolean; authenticated_at: string | null;
  last_read_ok_at: string | null; last_attempt_at: string | null; last_success_at: string | null; last_outcome: string | null;
  last_error: string | null; config: Record<string, unknown>; sources: ProviderSource[]; stored_messages: number; fetch_running: boolean }
export interface Polling { enabled: boolean; interval_minutes: number }
export interface ConnectionsStatus { accounts: ProviderAccount[]; polling: Polling; hiz_host: string; hiz_port: number; os_vault_available: boolean }

export interface AutoSettings { enabled: boolean; paused: boolean; model: string;
  worker_state: 'idle' | 'working' | 'offline' | 'model_missing' | 'error'; worker_detail: string; backoff_until: string | null; date_zone?: string }
export interface SourceRef { source_id: string; subject: string | null; sender: string | null; label: string; received_at: string;
  state: 'routine' | 'invalid_output' | 'failed' | 'cancelled' | 'truncated'; detail: string }
export interface DailySummary { settings: AutoSettings; inference_busy: boolean;
  queue: { queued: number; running: number; failed: number; cancelled: number }; sources_total: number; not_analysed: number;
  actionable_total: number; actionable: Candidate[]; needs_decision: number; updates_total: number; updates: Candidate[];
  routine_total: number; routine: SourceRef[]; attention_total: number; attention: SourceRef[]; all_analysed: boolean }
export interface BatchItem { id: string; payload_hash: string; title?: string; due_date?: string | null; due_time?: string | null; date_resolved?: boolean }
export interface BatchResult { committed: boolean; created: number; replayed: number;
  results: { id: string; ok: boolean; task_id: string | null; replayed: boolean; code: string | null; message: string | null }[] }

export type Transport = (cmd: CommandName, args?: Record<string, unknown>) => Promise<unknown>;

let transport: Transport | null = null;

/** Tests inject a labeled in-memory transport; production uses Tauri's invoke. */
export function setTransport(t: Transport | null): void {
  transport = t;
}

async function call<T>(cmd: CommandName, args?: Record<string, unknown>): Promise<T> {
  if (!transport) {
    if (!('__TAURI_INTERNALS__' in globalThis)) {
      throw { code: 'Unavailable', message: 'Desktop backend not available (running outside Tauri).' } satisfies IpcError;
    }
    const { invoke } = await import('@tauri-apps/api/core');
    transport = (c, a) => invoke(c, a);
  }
  return (await transport(cmd, args)) as T;
}

export const api = {
  emailDiscover:(email:string,preferred?:'google'|'microsoft')=>call<EmailProfile>('email_discover',{req:{email,preferred}}),
  emailConfigure:(profile_id:string,username?:string)=>call<string>('email_configure',{req:{profile_id,username}}),
  emailSignIn:(account_id:string,password?:string)=>call<string[]>('email_sign_in',{req:{account_id,password}}),
  localAiSetup:(action:'inspect'|'installer'|'open'|'download'|'cancel'='inspect',model?:string)=>call<LocalSetup>('local_ai_setup',{req:{action,model}}),
  assistantSummaryGet:()=>call<AssistantBrief>('assistant_summary_get'),
  aiSettings:(req:AiRequest={action:'get'})=>call<AiSettings>('ai_settings',{req}),
  assistantSummaryRefresh:()=>call<AssistantBrief>('assistant_summary_refresh'),
  assistantMemory:(action:'list'|'save'|'delete',note?:{id?:string;text?:string;expected_hash?:string;confirm?:boolean})=>call<AssistantNote[]>('assistant_memory',{req:{action,id:null,text:null,expected_hash:null,confirm:null,...note}}),
  appStatus: () => call<AppStatus>('app_status'),
  health: () => call<Health>('health'),
  settingsGet: (key: SettingKey) => call<Setting | null>('settings_get', { req: { key } }),
  settingsSet: (key: SettingKey, value: string, expected_revision: number) =>
    call<Setting>('settings_set', { req: { key, value, expected_revision } }),
  taskList: (status?: Task['status']) => call<Task[]>('task_list', { req: { status: status ?? null } }),
  taskCreate: (req: TaskCreate) => call<Task>('task_create', { req }),
  taskUpdate: (id: string, expected_revision: number, edits: { title?: string; notes?: string; due?: DueIn; clear_due?: boolean; schedule?: {effective_from:string;recurrence:RecurrenceIn} }) =>
    call<Task>('task_update', { req: { id, expected_revision, ...edits } }),
  taskComplete: (id: string, expected_revision: number) => call<Task>('task_complete', { req: { id, expected_revision } }),
  taskDelete: (id: string, expected_revision: number) => call<null>('task_delete', { req: { id, expected_revision, confirm: true } }),
  occurrenceEdit: (req: OccurrenceEdit) => call<Task>('occurrence_edit', { req }),
  scheduleChangeApply: (req: ScheduleApply) => call<ScheduleApplied>('schedule_change_apply', { req }),
  scheduleChangeUndo: (id: string, expected_revision: number) => call<ScheduleApplied>('schedule_change_undo', { req: { id, expected_revision } }),
  calendarDayLabels: (month:string) => call<{state:string;labels:Record<string,string>}>('calendar_day_labels',{req:{month}}),
  agendaList: (from: string, to: string) => call<AgendaItem[]>('agenda_list', { req: { from, to } }),
  eventList: (from: string, to: string) => call<CalEvent[]>('event_list', { req: { from, to } }),
  eventCreate: (req: EventCreate) => call<CalEvent>('event_create', { req }),
  eventDelete: (id: string, expected_revision: number) => call<null>('event_delete', { req: { id, expected_revision, confirm: true } }),
  proposalList: (status?: Proposal['status']) => call<Proposal[]>('proposal_list', { req: { status: status ?? null } }),
  proposalApprove: (id: string, payload_hash: string) => call<Proposal>('proposal_approve', { req: { id, payload_hash } }),
  proposalReject: (id: string) => call<Proposal>('proposal_reject', { req: { id } }),
  inboxOverview: () => call<InboxOverview>('inbox_overview'),
  assistantWorkflowSet: (enabled: boolean, account_id: string | null) => call<{ settings: AutoSettings; polling: Polling; selected_inbox: boolean }>('assistant_workflow_set', { req: { enabled, account_id } }),
  connectionsStatus: () => call<ConnectionsStatus>('connections_status'),
  hizConfigure: (account_id: string | null, username: string, initial_count: number) =>
    call<string>('hiz_configure', { req: { account_id, username, initial_count } }),
  /** The password is passed once to Rust, which stores it in the OS vault; the caller clears its field. */
  hizSignIn: (account_id: string, password: string) => call<string[]>('hiz_sign_in', { req: { account_id, password } }),
  teamsConfigure: (account_id: string | null, client_id: string, tenant: string, channels: boolean) =>
    call<string>('teams_configure', { req: { account_id, client_id, tenant, channels } }),
  teamsSignIn: (account_id: string) => call<string>('teams_sign_in', { req: { account_id } }),
  teamsDiscover: (account_id: string) => call<number>('teams_discover', { req: { account_id } }),
  selectSources: (account_id: string, selected: { source_kind: string; source_ref: string }[]) =>
    call<number>('connection_select_sources', { req: { account_id, selected } }),
  fetchNow: (account_id: string) => call<Record<string, unknown>>('connection_fetch_now', { req: { account_id } }),
  fetchCancel: (account_id: string) => call<boolean>('connection_fetch_cancel', { req: { account_id } }),
  disconnect: (account_id: string, purge: boolean) => call<boolean>('connection_disconnect', { req: { account_id, purge } }),
  pollingSet: (enabled: boolean, interval_minutes: number) => call<Polling>('polling_set', { req: { enabled, interval_minutes } }),
  sourceList: (limit = 50, offset = 0) => call<SourceSummary[]>('source_list', { req: { kind: null, limit, offset } }),
  sourceGet: (id: string) => call<SourceDetail>('source_get', { req: { id } }),
  sourceImportManual: (title: string, text: string) => call<SourceDetail>('source_import_manual', { req: { title, text, link: null } }),
  modelStatus: (model: string) => call<{ state: string; model: string; busy: boolean }>('model_status', { req: { model } }),
  sourceAnalyze: (id: string, model: string) => call<AnalysisResult>('source_analyze', { req: { id, model } }),
  sourceAnalyzeCancel: () => call<boolean>('source_analyze_cancel'),
  candidateApprove: (c: Candidate, edits: ApproveEdits = {}) =>
    call<{ task_id: string; replayed: boolean; candidate: Candidate }>('candidate_approve', { req: { id: c.id, payload_hash: c.payload_hash, ...edits } }),
  candidateReject: (c: Candidate) => call<Candidate>('candidate_reject', { req: { id: c.id, payload_hash: c.payload_hash } }),
  autoSettingsGet: () => call<AutoSettings>('auto_settings_get'),
  autoSettingsSet: (enabled: boolean, paused: boolean, model: string, date_zone?: string) =>
    call<AutoSettings>('auto_settings_set', { req: { enabled, paused, model, ...(date_zone ? { date_zone } : {}) } }),
  taskPlan: (id: string, expected_revision: number, plan_date: string, plan_time: string | null, duration_min: number | null) =>
    call<Task>('task_plan', { req: { id, expected_revision, plan_date, plan_time, duration_min, tz: plan_time ? Intl.DateTimeFormat().resolvedOptions().timeZone : null } }),
  taskUnplan: (id: string, expected_revision: number) => call<Task>('task_unplan', { req: { id, expected_revision } }),
  taskReopen: (id: string, expected_revision: number) => call<Task>('task_reopen', { req: { id, expected_revision } }),
  taskUndo: (id: string, expected_revision: number) => call<Task>('task_undo', { req: { id, expected_revision } }),
  modelList: () => call<{ state: 'ready' | 'not_running' | 'error'; models: string[] }>('model_list'),
  autoSummary: (actionable_offset = 0, limit = 25, updates_offset = 0, routine_offset = 0) =>
    call<DailySummary>('auto_summary', { req: { limit, actionable_offset, updates_offset, routine_offset } }),
  autoQueueCancel: () => call<number>('auto_queue_cancel'),
  autoQueueRetry: (source_id: string | null = null) => call<number>('auto_queue_retry', { req: { source_id } }),
  batchApprove: (items: BatchItem[]) => call<BatchResult>('candidate_batch_approve', { req: { items } }),
  suggestReschedules: (today: string) => call<Proposal[]>('planner_suggest_reschedules', { req: { today } }),
};

export function asIpcError(e: unknown): IpcError {
  if (e && typeof e === 'object' && 'code' in e && 'message' in e) return e as IpcError;
  return { code: 'Internal', message: String(e) };
}

/** The source's send day in the zone the resolver used (not the UTC day of the timestamp). */
export function sentDay(c: Pick<Candidate, 'date_anchor' | 'date_zone'>): string {
  if (!c.date_anchor) return '?';
  const d = new Date(c.date_anchor);
  if (Number.isNaN(d.getTime())) return '?';
  try {
    return new Intl.DateTimeFormat('en-CA', { timeZone: c.date_zone ?? undefined, year: 'numeric', month: '2-digit', day: '2-digit' }).format(d);
  } catch {
    return c.date_anchor.slice(0, 10);
  }
}
