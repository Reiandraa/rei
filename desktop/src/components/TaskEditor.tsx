import { useEffect, useRef, useState, type FormEvent } from 'react';
import { createPortal } from 'react-dom';
import { api, asIpcError, type DueIn, type Task } from '../ipc';
import { weekdays } from '../schedule';
export type EditorState = { mode: 'create' | 'edit' | 'plan'; task?: Task; day?: string; scheduled?: boolean };
const localDay = () => { const d = new Date(); return `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`; };
/** One editor for creating/editing a task and for its separate work plan. All saves use real IPC. */
export function TaskEditor({ editor, onClose, onSaved }: { editor: EditorState; onClose: () => void; onSaved: (saved?: Task) => Promise<void> }) {
  const { task, mode } = editor;
  const existing=task?.recurrence;
  const oldRule=Object.fromEntries((existing?.rrule.replace(/^RRULE:/,'')??'').split(';').filter(Boolean).map(s=>s.split('=')));
  const existingDays=oldRule.BYDAY?.split(',') ?? [weekdays[(new Date(`${existing?.dtstart_local.slice(0,10)??localDay()}T12:00:00`).getDay()+6)%7].code];
  const allDay=Boolean(existing && !existing.dtstart_local.includes('T'));
  const scheduleToday=existing?new Intl.DateTimeFormat('sv-SE',{timeZone:existing.tz,year:'numeric',month:'2-digit',day:'2-digit'}).format(new Date()):localDay();
  const earliestScheduleDate=[scheduleToday,task?.schedule_from??'',existing?.dtstart_local.slice(0,10)??''].sort().at(-1)!;
  const [title, setTitle] = useState(task?.title ?? '');
  const [notes, setNotes] = useState(task?.notes ?? '');
  const [date, setDate] = useState(task?.due_date ?? task?.due_at_local?.slice(0,10) ?? '');
  const [time, setTime] = useState(task?.due_at_local?.slice(11,16) ?? '');
  const [zone, setZone] = useState(existing?.tz ?? task?.due_tz ?? Intl.DateTimeFormat().resolvedOptions().timeZone ?? 'Europe/Berlin');
  const [plan, setPlan] = useState(task?.plan_date ?? editor.day ?? localDay());
  const [start, setStart] = useState(task?.plan_time ?? '');
  const [duration, setDuration] = useState(String(task?.plan_duration_min ?? 60));
  const [repeat, setRepeat] = useState(existing?oldRule.FREQ:editor.scheduled ? 'WEEKLY' : '');
  const [repeatTime, setRepeatTime] = useState(existing?.dtstart_local.slice(11,16) || '09:00');
  const [repeatStart, setRepeatStart] = useState(existing?earliestScheduleDate:localDay());
  const [repeatDays, setRepeatDays] = useState<string[]>(existing?existingDays:[weekdays[(new Date().getDay()+6)%7].code]);
  const [dayTimes, setDayTimes] = useState<Record<string, string>>(existing?.weekday_times??{});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ref = useRef<HTMLElement>(null);
  const closeRef = useRef(onClose); closeRef.current = onClose;
  const busyRef = useRef(busy); busyRef.current = busy;
  useEffect(() => {
    const trigger = document.activeElement as HTMLElement | null;
    const shell = document.querySelector('.shell');
    const wasInert = shell?.hasAttribute('inert');
    shell?.setAttribute('inert', '');
    ref.current?.querySelector<HTMLInputElement>('input')?.focus();
    const keys = (e: KeyboardEvent) => {
      if (e.key === 'Escape' && !busyRef.current) { e.preventDefault(); closeRef.current(); }
      if (e.key === 'Tab') {
        const focusable = [...(ref.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), textarea:not(:disabled), select:not(:disabled), summary') ?? [])].filter(el => !el.closest('details:not([open])') || el.tagName === 'SUMMARY');
        if (!focusable.length) { e.preventDefault(); return; }
        const first = focusable[0], last = focusable.at(-1);
        if (e.shiftKey && (document.activeElement === first || !ref.current?.contains(document.activeElement))) { e.preventDefault(); last?.focus(); }
        else if (!e.shiftKey && (document.activeElement === last || !ref.current?.contains(document.activeElement))) { e.preventDefault(); first?.focus(); }
      }
    };
    document.addEventListener('keydown', keys);
    return () => { document.removeEventListener('keydown', keys); if (!wasInert) shell?.removeAttribute('inert'); if (trigger?.isConnected) trigger.focus(); };
  }, []);
  async function submit(e: FormEvent) {
    e.preventDefault(); if (busy) return;
    setBusy(true); setError(null);
    try {
      let saved: Task | undefined;
      if (mode === 'plan' && task) {
        saved = await api.taskPlan(task.id, task.revision, plan, start || null, start ? Number(duration) : null);
      } else {
        const due: DueIn | undefined = date ? (time ? { kind: 'at', local: `${date}T${time}:00`, tz: zone } : { kind: 'date', date }) : undefined;
        const recurrence=repeat?{
          rrule:[`FREQ=${repeat}`,...(existing?Object.entries(oldRule).filter(([key])=>!['FREQ','BYDAY'].includes(key) && (key!=='BYMONTHDAY'||repeat==='MONTHLY')).map(([k,v])=>`${k}=${v}`):[]),...(repeat==='WEEKLY'?[`BYDAY=${weekdays.filter(d=>repeatDays.includes(d.code)).map(d=>d.code).join(',')}`]:[])].join(';'),
          tz:zone,dtstart_local:allDay?existing!.dtstart_local:`${existing?.dtstart_local.slice(0,10)??repeatStart}T${repeatTime}:00`,
          ...(repeat==='WEEKLY'&&!allDay?{weekday_times:Object.fromEntries(repeatDays.map(d=>[d,dayTimes[d]??repeatTime]))}:{})
        }:undefined;
        const scheduleChanged=Boolean(existing && (repeat!==oldRule.FREQ || repeatTime!==(existing.dtstart_local.slice(11,16)||'09:00') || (repeat==='WEEKLY' && (repeatDays.slice().sort().join()!==existingDays.slice().sort().join() || !allDay && repeatDays.some(d=>(dayTimes[d]??repeatTime)!==(existing.weekday_times?.[d]??existing.dtstart_local.slice(11,16)))))));
        if (task) {
          const oldDate = task.due_date ?? task.due_at_local?.slice(0,10) ?? '';
          const dueChanged = date !== oldDate || time !== (task.due_at_local?.slice(11,16) ?? '') || (Boolean(time) && zone !== task.due_tz);
          saved = await api.taskUpdate(task.id, task.revision, { title: title.trim(), notes, ...(existing && recurrence && scheduleChanged?{schedule:{effective_from:repeatStart,recurrence}}:{}), ...(dueChanged ? due ? { due } : { clear_due: true } : {}) });
        } else {
          saved = await api.taskCreate({title:title.trim(),...(notes?{notes}:{}),...(recurrence?{recurrence}:due?{due}:{})});
        }
      }
      await onSaved(saved); onClose();
    } catch (e) { setError(asIpcError(e).message); }
    finally { setBusy(false); }
  }
  const heading = mode === 'plan' ? `Plan ${task?.title}` : existing ? 'Edit schedule' : task ? 'Edit task' : 'Add task';
  return createPortal(<div className="editor-overlay"><section ref={ref} className="task-dialog" role="dialog" aria-modal="true" aria-labelledby="task-editor-heading">
    <div className="section-head"><h2 id="task-editor-heading">{heading}</h2><button type="button" aria-label="Close task editor" disabled={busy} onClick={onClose}>×</button></div>
    <form aria-label={mode === 'plan' ? `Plan ${task?.title}` : existing ? 'Edit schedule' : task ? 'Edit task' : 'New task'} onSubmit={e => void submit(e)}>
      <fieldset disabled={busy}>
      {mode === 'plan' ? <>
        <p className="muted">Choose when to work. Your deadline stays unchanged.</p>
        <label>Work on<input type="date" required value={plan} onChange={e => setPlan(e.target.value)} /></label>
        <div className="editor-fields"><label>Start (optional)<input type="time" value={start} onChange={e => setStart(e.target.value)} /></label>
        {start && <label>Minutes<input type="number" min={5} max={1440} required value={duration} onChange={e => setDuration(e.target.value)} /></label>}</div>
        {start && <p className="muted">Work time: {Intl.DateTimeFormat().resolvedOptions().timeZone}. No time is chosen automatically.</p>}
      </> : <>
        <label>What needs to be done?<input value={title} required maxLength={500} onChange={e => setTitle(e.target.value)} /></label>
        <label>Notes<textarea value={notes} rows={2} maxLength={10000} onChange={e => setNotes(e.target.value)} /></label>
        {!task?.recurrence && !repeat && <div className="editor-fields"><label>Due date (optional)<input type="date" value={date} onChange={e => { setDate(e.target.value); if (!e.target.value) setTime(''); }} /></label>
          <label>Due time (optional)<input type="time" disabled={!date || Boolean(repeat)} value={time} onChange={e => setTime(e.target.value)} /></label></div>}
        {!repeat && time && date && <label>Deadline time zone<input value={zone} required maxLength={64} onChange={e => setZone(e.target.value)} /></label>}
        {(mode === 'create' || existing) && <details open={Boolean(existing) || editor.scheduled || undefined}><summary>{existing?'Repeating schedule':'More options'}</summary>
          <label>Repeat<select value={repeat} onChange={e => setRepeat(e.target.value)}>{!existing && <option value="">Does not repeat</option>}<option value="DAILY">Every day</option><option value="WEEKLY">Every week</option><option value="MONTHLY">Every month</option></select></label>
          {repeat && <div className="repeat-options">
            <label>{existing?'Apply changes from':'Starting from'}<input type="date" min={existing?earliestScheduleDate:undefined} required value={repeatStart} onChange={e => setRepeatStart(e.target.value)} /></label>
            {repeat === 'WEEKLY' ? <>
              <div role="group" aria-label="Repeat on weekdays" className="weekday-picker">{weekdays.map(d => <button type="button" key={d.code} aria-label={d.name} aria-pressed={repeatDays.includes(d.code)} onClick={() => setRepeatDays(days => days.includes(d.code) ? days.filter(x => x !== d.code) : [...days,d.code])}>{d.short}</button>)}</div>
              {!repeatDays.length && <p role="status" className="muted">Choose at least one day.</p>}
              {!allDay && <div className="weekday-times">{weekdays.filter(d => repeatDays.includes(d.code)).map(d => <label key={d.code}>{d.name}<input aria-label={`${d.name} time`} type="time" required value={dayTimes[d.code] ?? repeatTime} onChange={e => setDayTimes(times => ({...times,[d.code]:e.target.value}))} /></label>)}</div>}
            </> : !allDay && <label>Time of day<input type="time" required value={repeatTime} onChange={e => setRepeatTime(e.target.value)} /></label>}
            <label>Schedule time zone<input value={zone} readOnly={Boolean(existing)} required maxLength={64} onChange={e => setZone(e.target.value)} /></label>
            <p className="muted">{repeat === 'WEEKLY' ? 'Each selected day keeps its own local time.' : 'Repeats at this local time.'}</p>
          </div>}
        </details>}
        <p className="muted">{task?.recurrence ? 'The repeating pattern changes from the date above. Earlier meetings, completed/skipped meetings and approved exceptions are kept. Title and notes apply to the routine.' : repeat ? 'Repeating tasks appear on their scheduled days. Complete or skip each occurrence in Calendar.' : 'A deadline is separate from your work plan. Plan the task after saving.'}</p>
      </>}
      </fieldset>
      {error && <p className="error" role="alert">{error}{' '}<small>Your changes have not been retried automatically. Close and reopen if the task changed elsewhere.</small></p>}
      <div className="dialog-actions"><button type="button" disabled={busy} onClick={onClose}>Cancel</button><button type="submit" className="primary" disabled={busy || (mode !== 'plan' && !title.trim()) || (repeat === 'WEEKLY' && repeatDays.length === 0)}>{busy ? 'Saving…' : mode === 'plan' ? 'Save plan' : existing ? 'Save schedule' : task ? 'Save task' : 'Add task'}</button></div>
    </form>
  </section></div>, document.body);
}
