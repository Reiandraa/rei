// Today, Tasks, Calendar and Review-changes screens. All data comes from the Rust backend through
// typed IPC; nothing is cached or invented in the renderer.
import { DeleteAction } from '../components/DeleteAction';
import { DailyReview } from './DailyReview';
import { TaskEditor, type EditorState } from '../components/TaskEditor';
import { DayTimeline } from '../components/DayTimeline';
import type { TimelineTaskAction } from '../components/TaskTimelineControls';
import { recurrenceLabel } from '../schedule';
export { CalendarScreen } from './MonthCalendar';
import { useCallback, useEffect, useRef, useState } from 'react';
import { api, asIpcError, type AgendaItem, type CalEvent, type DueIn, type Proposal, type Task } from '../ipc';
import { LONG_DATE, s } from '../strings';

const pad = (n: number) => String(n).padStart(2, '0');
export const isoDate = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
const addDays = (d: Date, n: number) => new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
const clock = (local: string) => local.replace('T', ' ').slice(0, 16);

const hhmm = (local: string) => local.slice(11, 16);
const dayLabel = (iso: string) => LONG_DATE.format(new Date(`${iso.slice(0, 10)}T12:00:00`));
const dueText = (d: DueIn) => (d.kind === 'date' ? dayLabel(d.date) : `${dayLabel(d.local)}, ${hhmm(d.local)}`);
const taskDue = (t: Task) =>
  t.due_date ? dayLabel(t.due_date) : t.due_at_local ? `${dayLabel(t.due_at_local)}, ${hhmm(t.due_at_local)}` : null;

function useLoad<T>(fn: () => Promise<T>, initial: T) {
  const [data, setData] = useState<T>(initial);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const reload = useCallback(async () => {
    try {
      setData(await fn());
      setError(null);
    } catch (e) {
      setError(asIpcError(e).message);
    } finally {
      setLoading(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    void reload();
  }, [reload]);
  return { data, error, loading, reload, setError };
}

function Errors({ message }: { message: string | null }) {
  return message ? <p role="alert" className="error">{message}</p> : null;
}

function Confirmation({ message }: { message: string | null }) {
  return message ? <p role="status" className="success">{message}</p> : null;
}

/** Runs a mutation, surfacing the backend error (e.g. revision Conflict) and reloading afterwards. */
function useAction(reload: () => Promise<void>, setError: (m: string | null) => void) {
  return async (op: () => Promise<unknown>): Promise<boolean> => {
    let failure: string | null = null;
    try {
      await op();
    } catch (e) {
      const err = asIpcError(e);
      failure = err.code === 'Conflict' ? s.conflict : err.message;
    }
    await reload(); // always re-read: the backend is the source of truth after success or refusal
    setError(failure); // reload clears errors on success; keep the refusal visible
    return failure === null;
  };
}

/** Shared task actions use backend revisions; a work plan never replaces the deadline. */
function TaskRows({ tasks, act, edit, deleted, sources = {} }: { tasks: Task[]; act: (op: () => Promise<unknown>) => Promise<boolean>; edit: (e: EditorState) => void; deleted: (t: Task) => void; sources?: Record<string, string> }) {
  const [busy, setBusy] = useState<string | null>(null);
  const run = async (t: Task, op: () => Promise<unknown>) => { if (busy) return; setBusy(t.id); try { await act(op); } finally { setBusy(null); } };
  const undoLabel: Record<string, string> = { complete: 'Undo complete', plan: 'Undo plan', unplan: 'Undo unplan' };
  return <ul className="task-list">{tasks.map(t => <li className={`task-row ${t.status === 'done' ? 'task-done' : ''}`} key={t.id}>
    {!t.recurrence && <button className="task-check" disabled={Boolean(busy)} aria-label={t.status === 'done' ? `Reopen ${t.title}` : s.tasks.complete(t.title)} aria-pressed={t.status === 'done'} onClick={() => void run(t, () => t.status === 'done' ? api.taskReopen(t.id, t.revision) : api.taskComplete(t.id, t.revision))}><span>{t.status === 'done' ? '✓' : ''}</span></button>}
    <div className="task-body"><button className="task-title" aria-label={`Edit ${t.title}`} onClick={() => edit({ mode: 'edit', task: t })}>{t.title}</button>
      <div className="task-meta"><span className={t.recurrence ? 'routine-meta' : 'deadline'}>{t.recurrence ? t.status === 'cancelled' ? 'Cancelled routine' : 'Repeating routine' : `${taskDue(t) ? `Due ${taskDue(t)}` : s.tasks.noDue} · ${s.tasks.status[t.status] ?? t.status}`}</span>
        <span>{t.recurrence ? `${recurrenceLabel(t.recurrence)}${t.schedule_from?` · From ${t.schedule_from}`:''}${t.has_schedule_changes?' · Includes approved changes; see Calendar':''}` : t.plan_date ? `Planned ${t.plan_date}${t.plan_time ? ` ${t.plan_time}` : ''}${t.plan_duration_min ? ` (${t.plan_duration_min} min)` : ''}` : 'Not planned'}</span>
        {sources[t.id] && <a href={`#/inbox?source=${sources[t.id]}`}>source</a>}
      </div>
      <div className="task-actions">
        {t.recurrence && <><button aria-label={`Edit schedule: ${t.title}`} onClick={()=>edit({mode:'edit',task:t})}>Edit schedule</button><a href="#/calendar">View meetings</a></>}
        {!t.recurrence && t.status !== 'done' && <button className="task-plan-button" disabled={Boolean(busy)} aria-label={t.plan_date ? `Move ${t.title}` : `Plan ${t.title}`} onClick={() => edit({ mode: 'plan', task: t })}>{t.plan_date ? 'Move' : 'Plan'}</button>}
        {t.plan_date && !t.recurrence && <button disabled={Boolean(busy)} aria-label={`Unplan ${t.title}`} onClick={() => void run(t, () => api.taskUnplan(t.id, t.revision))}>Remove from Today</button>}
        {t.undo_action && t.undo_action !== 'delete' && <button disabled={Boolean(busy)} onClick={() => void run(t, () => api.taskUndo(t.id, t.revision))}>{undoLabel[t.undo_action]}</button>}
        <DeleteAction title={t.title} kind={t.recurrence?'routine':'task'} disabled={Boolean(busy)} onConfirm={async()=>{if(busy)return false;setBusy(t.id);try{return await act(async()=>{await api.taskDelete(t.id,t.revision);if(!t.recurrence)deleted(t);});}finally{setBusy(null);}}}/>
      </div>
    </div>
  </li>)}</ul>;
}

export function TasksScreen() {
  const { data: tasks, error, loading, reload, setError } = useLoad(() => api.taskList(), [] as Task[]);
  const act = useAction(reload, setError);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [deleted, setDeleted] = useState<Task | null>(null);
  const [saved, setSaved] = useState<string | null>(null);
  const [view, setView] = useState<'once'|'scheduled'>('once');
  const oneTime = tasks.filter(t=>!t.recurrence);
  const scheduled = tasks.filter(t=>Boolean(t.recurrence));
  const visible = view === 'once' ? oneTime : scheduled;
  return <>
    <div className="page-heading"><p className="muted">{view==='once'?'Assignments, deadlines and things you do once.':'Your repeating routines. Each meeting keeps its own completion and changes.'}</p><button className="primary" onClick={() => setEditor({ mode: 'create', scheduled:view==='scheduled' })}>{view==='once'?'+ Add task':'+ Add routine'}</button></div>
    <div className="task-tabs" role="tablist" aria-label="Task types">{(['once','scheduled'] as const).map(v=><button key={v} id={`task-tab-${v}`} role="tab" aria-selected={view===v} aria-controls="task-type-panel" tabIndex={view===v?0:-1} onKeyDown={e=>{if(['ArrowLeft','ArrowRight','Home','End'].includes(e.key)){e.preventDefault();const next=e.key==='Home'?'once':e.key==='End'?'scheduled':view==='once'?'scheduled':'once';setView(next);document.getElementById(`task-tab-${next}`)?.focus();}}} onClick={()=>setView(v)}>{v==='once'?'One-time':'Scheduled'} <span>{v==='once'?oneTime.length:scheduled.length}</span></button>)}</div>
    <Confirmation message={saved} /><Errors message={error} />
    {deleted && <p className="undo-toast" role="status">Deleted {deleted.title}. <button onClick={() => void act(async () => { await api.taskUndo(deleted.id, deleted.revision + 1); setDeleted(null); })}>Undo delete</button></p>}
    <section id="task-type-panel" role="tabpanel" aria-labelledby={`task-tab-${view}`}>{loading ? <p role="status">{s.loading}</p> : visible.length === 0 ? <p role="status" className="empty-state">{view==='once'?'No tasks yet. Add a task or approve suggestions from your messages.':'No scheduled routines yet. Add a routine and choose its repeating days.'}</p> : <TaskRows tasks={visible} act={act} edit={setEditor} deleted={setDeleted} />}</section>
    {editor && <TaskEditor editor={editor} onClose={() => setEditor(null)} onSaved={async saved => { await reload(); if(editor.mode==='create')setView(saved?.recurrence?'scheduled':'once'); setSaved(editor.mode === 'create' ? 'Task saved.' : 'Changes saved.'); }} />}
  </>;
}

/** One row per task, even when it has both a future work plan and a deadline. */
function UpcomingTasks({tasks,edit}:{tasks:Task[];edit:(editor:EditorState)=>void}) {
  return <ul className="rows upcoming-tasks">{tasks.map(t=><li key={t.id}><div className="row-main">
    <button className="task-title" aria-label={`Edit ${t.title}`} onClick={()=>edit({mode:'edit',task:t})}>{t.title}</button>
    <div className="task-meta">
      {t.plan_date && <span>Planned {dayLabel(t.plan_date)}{t.plan_time?`, ${t.plan_time}`:''}</span>}
      {taskDue(t) && <span className="deadline">Due {taskDue(t)}</span>}
    </div>
  </div></li>)}</ul>;
}

export function TodayScreen() {
  const [day, setDay] = useState(() => isoDate(new Date()));
  const [items, setItems] = useState<AgendaItem[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [events, setEvents] = useState<CalEvent[]>([]);
  const [sourceLinks, setSourceLinks] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [editor, setEditor] = useState<EditorState | null>(null);
  const [deleted, setDeleted] = useState<Task | null>(null);
  const [suggested, setSuggested] = useState<number | null>(null);
  const generation = useRef(0);
  const selectedDay = useRef(day); selectedDay.current = day;
  const invalidate = useCallback(() => { generation.current++; }, []);
  const reload = useCallback(async () => {
    const version = ++generation.current;
    const day = selectedDay.current;
    const end = isoDate(addDays(new Date(`${day}T12:00:00`), 6));
    try {
      const [agenda, nextTasks, nextEvents, inbox] = await Promise.all([api.agendaList(day, end), api.taskList(), api.eventList(day, end), api.inboxOverview().catch(() => null)]);
      if (version === generation.current) { setItems(agenda); setTasks(nextTasks); setEvents(nextEvents); setSourceLinks(Object.fromEntries((inbox?.source_tasks ?? []).map(t => [t.task_id,t.source_id]))); setError(null); }
    } catch (e) { if (version === generation.current) setError(asIpcError(e).message); }
    finally { if (version === generation.current) setLoading(false); }
  }, []);
  useEffect(() => { setLoading(true); void reload(); return invalidate; }, [day, reload, invalidate]);
  const act = useAction(reload, setError);
  const chosen = new Date(`${day}T12:00:00`);
  const week = Array.from({ length: 7 }, (_, n) => addDays(chosen, n - ((chosen.getDay()+6)%7)));
  const oneTime = tasks.filter(t => !t.recurrence && t.status !== 'cancelled');
  const planned = oneTime.filter(t => t.plan_date === day);
  const deadlines = oneTime.filter(t => (t.due_date ?? t.due_at_local?.slice(0,10)) === day && t.plan_date !== day && t.status !== 'done');
  const unscheduled = oneTime.filter(t => !t.plan_date && (t.due_date ?? t.due_at_local?.slice(0,10)) !== day && t.status !== 'done');
  const todayItems = items.filter(i => {
    if (i.kind !== 'event') return i.local_start.slice(0,10) === day;
    const e = events.find(e => e.id === i.id);
    return e ? e.start_local.slice(0,10) <= day && e.end_local > (e.all_day ? day : `${day}T00:00:00`) : i.local_start.slice(0,10) === day;
  });
  const upcomingEnd = isoDate(addDays(chosen,6));
  const upcoming = oneTime.filter(t=>t.status!=='done').flatMap(task=>{
    const next = [task.plan_date?`${task.plan_date}T${task.plan_time??'00:00'}`:null, task.due_date?`${task.due_date}T00:00`:task.due_at_local]
      .filter((date):date is string=>Boolean(date && date.slice(0,10)>day && date.slice(0,10)<=upcomingEnd)).sort()[0];
    return next?[{task,next}]:[];
  }).sort((a,b)=>a.next.localeCompare(b.next)||a.task.title.localeCompare(b.task.title)).map(entry=>entry.task);
  const editOccurrence = (i: AgendaItem, action: 'done' | 'skip' | 'reset') => act(() => api.occurrenceEdit({ task_id: i.id, expected_revision: i.task_revision!, scope: 'this', action, original_local: i.original_local! }));
  const editTask = (e: EditorState) => setEditor({ ...e, day });
  const timelineTaskAction = (t:Task, action:TimelineTaskAction) => act(async()=>{
    if(action==='complete')await api.taskComplete(t.id,t.revision);
    else if(action==='reopen')await api.taskReopen(t.id,t.revision);
    else if(action==='unplan')await api.taskUnplan(t.id,t.revision);
    else {await api.taskDelete(t.id,t.revision);setDeleted(t);}
  });
  return <>
    <div className="page-heading"><div><p className="date-line">{LONG_DATE.format(chosen)}</p><p className="muted">A little structure. More room for your day.</p></div><div className="actions"><button className="primary" onClick={() => setEditor({ mode: 'create' })}>+ Add task</button><a className="button" href="#/calendar">Open calendar</a></div></div>
    <div className="week-strip" aria-label="Choose a day"><button aria-label="Previous week" onClick={() => setDay(isoDate(addDays(chosen,-7)))}>‹</button>{week.map(d => <button key={isoDate(d)} aria-pressed={isoDate(d) === day} aria-label={LONG_DATE.format(d)} onClick={() => setDay(isoDate(d))}><small>{d.toLocaleDateString('en-GB',{weekday:'short'})}</small><span>{d.getDate()}</span></button>)}<button aria-label="Next week" onClick={() => setDay(isoDate(addDays(chosen,7)))}>›</button><button className="week-today" onClick={() => setDay(isoDate(new Date()))}>Today</button></div>
    <Errors message={error} />
    {deleted && <p className="undo-toast" role="status">Deleted {deleted.title}. <button onClick={() => void act(async () => { await api.taskUndo(deleted.id, deleted.revision+1); setDeleted(null); })}>Undo delete</button></p>}
    <div className="planner-columns"><div className="planner-tasks">
      <DailyReview onTasksChanged={reload} compact extraReview={<ProposalsScreen/>} />
      <section aria-label="Planned tasks"><div className="section-head"><h2>Planned for this day</h2><span className="muted">{planned.length} tasks</span></div>
        {loading ? <p role="status">Loading…</p> : planned.length ? <TaskRows tasks={planned} act={act} edit={editTask} deleted={setDeleted} sources={sourceLinks} /> : <p className="empty-state">Nothing planned yet. Choose a task below and give it some time.</p>}
      </section>
      {deadlines.length > 0 && <section className="planner-section" aria-label="Deadlines"><div className="section-head"><h2>Due this day</h2><span className="muted">Deadlines, separate from work time</span></div><TaskRows tasks={deadlines} act={act} edit={editTask} deleted={setDeleted} sources={sourceLinks} /></section>}
      <section className="planner-section" aria-label="Unscheduled tasks"><div className="section-head"><h2>Not planned yet</h2><a href="#/tasks">All tasks →</a></div>
        {unscheduled.length ? <TaskRows tasks={unscheduled} act={act} edit={editTask} deleted={setDeleted} sources={sourceLinks} /> : <p className="muted">No open tasks waiting for a work plan.</p>}
      </section>
      <details className="planner-section"><summary>{s.today.upcomingSection}</summary>{loading?<p role="status">Loading…</p>:upcoming.length ? <UpcomingTasks tasks={upcoming} edit={editTask} /> : <p className="muted">{s.today.upcomingEmpty}</p>}</details>
      <details className="planner-section"><summary>Review overdue tasks</summary><button onClick={() => void act(async () => setSuggested((await api.suggestReschedules(day)).length))}>{s.today.suggest}</button>{suggested !== null && <p role="status">{suggested === 0 ? s.today.suggestNone : s.today.suggestSome(suggested)}</p>}</details>
    </div><aside className="calendar-pane" aria-label="Daily timeline"><div className="section-head"><h2>Your day</h2><span className="muted">{chosen.toLocaleDateString('en-GB',{day:'numeric',month:'short'})}</span></div><DayTimeline day={day} items={todayItems} tasks={tasks} events={events} editTask={editTask} editOccurrence={editOccurrence} onTaskAction={timelineTaskAction} /><p className="pane-note">Work blocks and calendar events share this timeline. Deadlines are marked separately. Local times retain their saved time zones.</p></aside></div>
    {editor && <TaskEditor editor={editor} onClose={() => setEditor(null)} onSaved={reload} />}
  </>;
}


function ChangeSummary({ p, current }: { p: Proposal; current: Task | undefined }) {
  const d = p.payload.due;
  if (!d) return <p>{s.review.otherChange}: {p.action}</p>;
  const now = current ? (taskDue(current) ?? s.review.noDue) : s.review.unknownCurrent;
  return (
    <dl className="change">
      <dt>{s.review.dueChange}, {s.review.before.toLowerCase()}</dt><dd>{now}</dd>
      <dt>{s.review.dueChange}, {s.review.after.toLowerCase()}</dt><dd>{dueText(d)}</dd>
    </dl>
  );
}

export function ProposalsScreen() {
  const { data: all, error, loading, reload, setError } = useLoad(() => api.proposalList(), [] as Proposal[]);
  const taskState = useLoad(() => api.taskList(), [] as Task[]);
  const { reload: reloadTasks } = taskState;
  const reloadAll = useCallback(async () => { await Promise.all([reload(), reloadTasks()]); }, [reload, reloadTasks]);
  const act = useAction(reloadAll, setError);
  const pending = all.filter((p) => p.status === 'Pending');
  const past = all.filter((p) => p.status !== 'Pending');
  const titleOf = (p: Proposal) => p.target_title ?? s.review.unknownTask;
  return (
    <>
      <p className="muted">{s.review.intro}</p>
      <Errors message={error} />
      <section className="card" aria-label={s.review.pendingLabel}>
        {loading ? <p role="status">{s.loading}</p> : pending.length === 0 ? (
          <p role="status" className="muted">{s.review.empty}</p>
        ) : (
          <ul className="proposals">
            {pending.map((p) => (
              <li key={p.id}>
                <h2>{titleOf(p)}</h2>
                <ChangeSummary p={p} current={taskState.data.find((t) => t.id === p.target_id)} />
                {p.rationale && <p><strong>{s.review.why}:</strong> {p.rationale}</p>}
                <p className="muted">{s.review.expires(clock(p.expires_at.replace('Z', '')))}</p>
                <div className="actions">
                  <button className="primary" onClick={() => void act(() => api.proposalApprove(p.id, p.payload_hash))}>{s.review.approve(titleOf(p))}</button>
                  <button onClick={() => void act(() => api.proposalReject(p.id))}>{s.review.keep(titleOf(p))}</button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
      {past.length > 0 && (
        <section className="card" aria-label={s.review.historyLabel}>
          <h2>{s.review.historyLabel}</h2>
          <ul className="rows">
            {past.map((p) => (
              <li key={p.id}>
                <span className="row-main">
                  {titleOf(p)}{p.payload.due ? `: due date to ${dueText(p.payload.due)}` : ''} — <span className="muted">{s.review.status[p.status] ?? p.status}</span>
                </span>
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}
