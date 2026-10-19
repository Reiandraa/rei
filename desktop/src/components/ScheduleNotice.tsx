import { useEffect, useState, type FormEvent } from 'react';
import { api, asIpcError, type Candidate, type ScheduleApplied, type Task } from '../ipc';

export function isScheduleNotice(c: Pick<Candidate,'excerpt'|'title'>): boolean {
  if(/\b(abgabe|einreichen|submit|assignment|deadline|frist)\b/i.test(c.excerpt))return false;
  const s=`${c.excerpt} ${c.title}`.toLowerCase();
  return /(lecture|vorlesung|seminar|class|meeting|raum|room|ruangan|kelas|jadwal|termin)/.test(s)
    && /(moved|verschob|verlegt|änder|aender|change|statt|pindah|später|spaeter|later|khusus|nur diesmal|entfällt|cancelled|canceled)/.test(s);
}
export function suggestedScope(excerpt:string): ''|'this'|'following' {
  const s=excerpt.toLowerCase();
  const bounded=/(vorübergehend|temporary|temporarily|until|bis zum|diese woche|this week)/.test(s);
  const once=/(only this|only on|just this|nur (diesen|diese|diesmal|am|morgen)|einmalig|khusus|hanya)/.test(s);
  const following=/(dauerhaft|ab sofort|ab jetzt|künftig|zukünftig|from now on|mulai sekarang)/.test(s);
  return following && !once && !bounded?'following':once && !following?'this':'';
}
export function suggestedRoom(excerpt:string): string {
  const matches=[...excerpt.matchAll(/(?:\bRaum|\broom|\bruangan)\s+([\p{L}\d][\p{L}\d._/-]*)/giu)];
  const room=matches.length===1?matches[0][1].replace(/[.,;]+$/,''):'';
  return /^(change|changed|changes|is|wird|ist|has|will|now|new|to|the)$/i.test(room)?'':room;
}
const normal=(s:string)=>s.toLowerCase().normalize('NFKC').split(/[^\p{L}\d]+/u).filter(w=>w.length>=4);

function ChangeForm({candidate:c,onApplied,onClose}:{candidate:Candidate;onApplied:(r:ScheduleApplied,label:string)=>Promise<void>;onClose:()=>void}) {
  const [tasks,setTasks]=useState<Task[]>([]);
  const [target,setTarget]=useState('');
  const [scope,setScope]=useState<''|'this'|'following'>(()=>suggestedScope(c.excerpt));
  const [date,setDate]=useState(c.due_date ?? '');
  const [time,setTime]=useState(c.due_time ?? '');
  const [room,setRoom]=useState(()=>suggestedRoom(c.excerpt));
  const [loading,setLoading]=useState(true);
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string|null>(null);
  useEffect(()=>{
    let current=true;
    api.taskList().then(all=>{
      if(!current)return;
      const routines=all.filter(t=>t.recurrence && t.status!=='cancelled');
      setTasks(routines);
      const words=normal(`${c.source_subject ?? ''} ${c.excerpt}`);
      const matches=routines.filter(t=>{const title=normal(t.title);return title.length>0 && title.every(w=>words.includes(w));});
      if(matches.length===1)setTarget(matches[0].id);
    }).catch(e=>{if(current)setError(asIpcError(e).message);}).finally(()=>{if(current)setLoading(false);});
    return ()=>{current=false;};
  },[c.excerpt,c.source_subject]);
  const task=tasks.find(t=>t.id===target);
  async function submit(e:FormEvent) {
    e.preventDefault();if(!task || !scope || busy)return;
    setBusy(true);setError(null);
    try {
      const result=await api.scheduleChangeApply({candidate_id:c.id,payload_hash:c.payload_hash,task_id:task.id,expected_revision:task.revision,scope,date,new_time:time||null,location:room.trim()||null});
      await onApplied(result,`${task.title}: ${scope==='this'?'only this meeting':'this weekday from this date onwards'} · ${date}`);
    } catch(e){setError(asIpcError(e).message);}
    finally{setBusy(false);}
  }
  return <form className="schedule-review" aria-label={`Schedule change: ${c.title}`} onSubmit={e=>void submit(e)}>
    <p className="overline">CHANGE AN EXISTING ROUTINE</p><blockquote>{c.excerpt}</blockquote>
    {loading?<p role="status">Loading routines…</p>:tasks.length===0?<p>No repeating routine is saved yet. Add it under Tasks → Scheduled, then return to this notice.</p>:<fieldset disabled={busy || c.expired || c.source_state!=='current'}>
      <label>Routine<select required value={target} onChange={e=>setTarget(e.target.value)}><option value="">Choose the affected routine</option>{tasks.map(t=><option key={t.id} value={t.id}>{t.title}</option>)}</select></label>
      <label>Applies to<select required value={scope} onChange={e=>setScope(e.target.value as typeof scope)}><option value="">Choose the scope</option><option value="this">Only this meeting</option><option value="following">This weekday from this date onwards</option></select></label>
      <div className="editor-fields"><label>{scope==='following'?'First affected meeting':'Meeting date'}<input required type="date" value={date} onChange={e=>setDate(e.target.value)} /></label><label>New time (optional)<input type="time" value={time} onChange={e=>setTime(e.target.value)} /></label></div>
      <label>New room (optional)<input value={room} maxLength={120} onChange={e=>setRoom(e.target.value)} /></label>
      <p className="muted">{task?.recurrence?.tz ?? 'Choose a routine to see its time zone.'} · Empty time or room keeps its current value. {scope==='following'?'Only this weekday changes; other weekdays keep their schedule.':'Other meetings keep their schedule.'}</p>
      <button className="primary" type="submit" disabled={!task || !scope || !date || (!time && !room.trim())}>{busy?'Applying…':'Apply schedule change'}</button>
    </fieldset>}
    {(c.expired || c.source_state!=='current') && <p role="alert">This notice expired or its source changed. Review the current message first.</p>}
    {error && <p role="alert">{error} Close and reopen this review to refresh. Your approval was not retried automatically.</p>}
    <button type="button" disabled={busy} onClick={onClose}>Close review</button>
  </form>;
}

export function ScheduleNotice({candidate,onApplied}:{candidate:Candidate;onApplied:(r:ScheduleApplied,label:string)=>Promise<void>}) {
  const [open,setOpen]=useState(false);
  if(/(entfällt|cancelled|canceled)/i.test(candidate.excerpt))return <a href="#/calendar">Review cancellation in Calendar</a>;
  return <div className="schedule-notice"><button type="button" onClick={()=>setOpen(!open)}>{open?'Hide schedule review':'Update schedule'}</button>{open && <ChangeForm key={`${candidate.id}:${candidate.payload_hash}`} candidate={candidate} onApplied={onApplied} onClose={()=>setOpen(false)} />}</div>;
}
