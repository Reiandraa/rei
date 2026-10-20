// SYNTHETIC in-memory transport for jsdom UI tests. It imitates the Rust command contract
// (revision checks) but is NOT the Rust backend; Rust is covered by cargo tests.
import type { CommandName, Transport } from '../../src/ipc';

type Row = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

export function syntheticBackend() {
  const store = new Map<string, { value: unknown; revision: number }>();
  // Synthetic planner fixtures; tests push rows directly.
  const tasks: Row[] = [];
  const agenda: Row[] = [];
  const undo = new Map<string, Row>();
  const proposals: Row[] = [];
  const calls: { cmd: CommandName; args?: Record<string, unknown> }[] = [];
  const transport: Transport = async (cmd, args) => {
    calls.push({ cmd, args });
    const req = (args as { req?: Row } | undefined)?.req;
    switch (cmd) {
      case 'local_ai_setup':
        if(req?.action==='inspect')return {platform:'windows',installed:false,memory_gb:8,recommended_model:'qwen3.5:2b',download:{state:'idle',model:'',detail:'',completed:0,total:0}};
        throw {code:'Unavailable',message:'Setup side effects are not implemented in this general synthetic fixture.'};
      case 'ai_settings':
        if(req?.action==='get')return {revision:0,provider:'local',model:'synthetic-model',local_model:'synthetic-model',openai_saved:false,anthropic_saved:false,daily_limit:50,used_today:0,available_models:[]};
        throw {code:'Validation',message:'No API action implemented in this general synthetic fixture.'};
      case 'assistant_summary_get': case 'assistant_summary_refresh':
        return {state:'idle',enabled:false,model:'synthetic-model',updated_at:null,detail:'',total:0,scope_limit:100,pending:0,routine:0,entries:[]};
      case 'assistant_memory': return [];
      case 'app_status':
        return { ipc_version: 1, schema_version: 1, lock_state: 'unlocked', device_id: 'synthetic-device' };
      case 'health':
        return { database_ok: true, schema_version: 1 };
      case 'settings_get': {
        const s = store.get(req!.key as string);
        return s ? { key: req!.key, ...s } : null;
      }
      case 'auto_settings_get': return { enabled: false, paused: false, model: 'qwen3.5:9b', worker_state: 'idle', worker_detail: '', backoff_until: null, date_zone: 'Europe/Berlin' };
      case 'inbox_overview': return { connections: [], source_tasks: [], attention: [], sources_total: 0, unanalysed: 0, pending_candidates: 0, live_fetch_available: false };
      case 'task_list':
        return tasks.map(t => ({...t}));
      case 'task_create': {
        const r = args as { req: Row };
        const t = {
          id: `t${tasks.length + 1}`, revision: 1, status: 'todo', notes: '', priority: 0, energy: null, duration_min: null,
          due_date: r.req.due?.date ?? null, due_at_utc: null, due_tz: null, due_at_local: null,
          recurrence: r.req.recurrence ? { series_id: 's', ...r.req.recurrence } : null, reminder_offsets_min: [],
          created_at: 'x', updated_at: 'x', title: r.req.title,
        };
        tasks.push(t);
        return t;
      }
      case 'task_update': {
        const t = tasks.find(x => x.id === req!.id)!;
        if (t.revision !== req!.expected_revision) throw { code: 'Conflict', message: 'revision mismatch' };
        if (req!.title !== undefined) t.title = req!.title;
        if (req!.notes !== undefined) t.notes = req!.notes;
        if(req!.schedule){t.recurrence={...t.recurrence,...req!.schedule.recurrence};t.schedule_from=req!.schedule.effective_from;}
        if (req!.clear_due) { t.due_date = null; t.due_at_local = null; t.due_tz = null; }
        if (req!.due) { t.due_date = req!.due.date ?? null; t.due_at_local = req!.due.local ?? null; t.due_tz = req!.due.tz ?? null; }
        t.revision++; return { ...t };
      }
      case 'task_plan': case 'task_unplan': case 'task_undo': {
        const t = tasks.find(x => x.id === req!.id)!;
        if (t.revision !== req!.expected_revision) throw { code: 'Conflict', message: 'revision mismatch' };
        if (cmd === 'task_undo') { const previous = undo.get(t.id); if (!previous) throw { code: 'Conflict', message: 'nothing to undo' }; Object.assign(t,previous,{revision:t.revision+1,undo_action:null}); undo.delete(t.id); }
        else { undo.set(t.id,{...t}); Object.assign(t,{ plan_date: cmd === 'task_plan' ? req!.plan_date : null, plan_time: cmd === 'task_plan' ? req!.plan_time : null, plan_duration_min: cmd === 'task_plan' ? req!.duration_min : null, plan_tz: cmd === 'task_plan' ? req!.tz : null, undo_action: cmd === 'task_plan' ? 'plan' : 'unplan', revision:t.revision+1 }); }
        return { ...t };
      }
      case 'task_complete': case 'task_reopen': {
        const t = tasks.find((x) => x.id === req!.id)!;
        if (t.revision !== req!.expected_revision) throw { code: 'Conflict', message: 'revision mismatch', current_revision: t.revision };
        t.status = cmd === 'task_complete' ? 'done' : 'todo';
        t.revision += 1;
        return t;
      }
      case 'task_delete':
        tasks.splice(tasks.findIndex((x) => x.id === req!.id), 1);
        return null;
      case 'agenda_list': {
        // Imitates the backend: dated tasks appear on their day (all-day), next to any fixture rows.
        const dated = tasks
          .filter((x) => x.due_date && x.due_date >= (req!.from as string) && x.due_date <= (req!.to as string))
          .map((x) => ({
            kind: 'task', id: x.id, title: x.title, local_start: `${x.due_date}T00:00:00`, all_day: true, state: x.status,
            original_local: null, task_revision: x.revision, recurring: false, read_only: false, conflict: false,
          }));
        const planned = tasks.filter(x => x.plan_date && x.plan_date >= req!.from && x.plan_date <= req!.to).map(x => ({ kind: 'plan', id: x.id, title: x.title, local_start: x.plan_time ? `${x.plan_date}T${x.plan_time}:00` : x.plan_date, all_day: !x.plan_time, state: x.status, original_local: null, task_revision: x.revision, recurring: false, read_only: false, conflict: false }));
        return [...dated, ...planned, ...agenda.map((a) => ({ ...a }))];
      }
      case 'occurrence_edit': {
        const a = agenda.find((x) => x.id === req!.task_id)!;
        a.state = ({ done: 'Done', skip: 'Skipped', reset: 'Normal' } as Row)[req!.action as string] ?? 'Normal';
        return {};
      }
      case 'event_list':
        return [];
      case 'planner_suggest_reschedules':
        return proposals.filter((p) => p.status === 'Pending');
      case 'proposal_list':
        return [...proposals];
      case 'proposal_approve': {
        const p = proposals.find((x) => x.id === req!.id)!;
        if (p.payload_hash !== req!.payload_hash) throw { code: 'ApprovalInvalid', message: 'payload hash mismatch' };
        p.status = 'Executed';
        return p;
      }
      case 'proposal_reject': {
        const p = proposals.find((x) => x.id === req!.id)!;
        p.status = 'Rejected';
        return p;
      }
      case 'settings_set': {
        const cur = store.get(req!.key as string)?.revision ?? 0;
        if (cur !== req!.expected_revision) {
          throw { code: 'Conflict', message: 'revision mismatch', current_revision: cur };
        }
        store.set(req!.key as string, { value: req!.value, revision: cur + 1 });
        return { key: req!.key, value: req!.value, revision: cur + 1 };
      }
      default:
        throw { code: 'Unavailable', message: `synthetic backend does not implement ${cmd}` };
    }
  };
  return { transport, store, calls, tasks, agenda, proposals };
}
