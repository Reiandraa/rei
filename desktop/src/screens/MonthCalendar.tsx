import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { api, asIpcError, type AgendaItem, type CalEvent, type Task } from '../ipc';
import { DeleteAction } from '../components/DeleteAction';
import { OccurrenceControls } from '../components/OccurrenceControls';
import { TaskEditor, type EditorState } from '../components/TaskEditor';
import { dateKey, dayOverview, entriesForDay, monthDays, weekdays } from '../schedule';

const longDay = (day: string) => new Date(`${day}T12:00:00`).toLocaleDateString('en-GB',{weekday:'long',day:'numeric',month:'long',year:'numeric'});
const kindLabel = (kind: AgendaItem['kind']) => ({event:'Event',occurrence:'Repeating task',plan:'Work block',task:'Deadline'})[kind];
type Data = {month: string; items: AgendaItem[]; events: CalEvent[]; tasks: Task[]};
type DayLabel = {label:string;signature:string};
/** Keep a label visible only while this date's saved classification inputs still match. */
function daySignature(data:Data,day:string,entries=entriesForDay(data.items,data.events,day)) {
  return JSON.stringify(entries.map(i=>{
    const task=data.tasks.find(t=>i.kind!=='event' && t.id===i.id);
    const event=data.events.find(e=>i.kind==='event' && e.id===i.id);
    return [i.kind,i.id,i.title,i.local_start,i.all_day,i.location,event?.end_local,event?.tz,
      i.kind==='plan'?task?.plan_duration_min:task?.duration_min,
      i.kind==='plan'?task?.plan_tz:i.kind==='occurrence'?task?.recurrence?.tz:task?.due_tz];
  }));
}

/** Saved commitments remain visible alongside advisory labels from local inference. */
export function CalendarScreen() {
  const today = dateKey(new Date());
  const [month,setMonth] = useState(today.slice(0,7));
  const [selected,setSelected] = useState(today);
  const [data,setData] = useState<Data>({month:'',items:[],events:[],tasks:[]});
  const [dayLabels,setDayLabels]=useState<Record<string,DayLabel>>({});
  const [labelState,setLabelState]=useState('waiting');
  const [loading,setLoading] = useState(true);
  const [error,setError] = useState<string|null>(null);
  const [busy,setBusy] = useState(false);
  const [saved,setSaved] = useState<string|null>(null);
  const [adding,setAdding] = useState(false);
  const [title,setTitle] = useState('');
  const [eventDate,setEventDate] = useState('');
  const [editor,setEditor] = useState<EditorState|null>(null);
  const generation = useRef(0);
  const invalidate = useCallback(()=>{generation.current++;},[]);
  const currentMonth = useRef(month); currentMonth.current=month;
  const reload = useCallback(async () => {
    const version=++generation.current;
    setLoading(true);
    const days=monthDays(month).filter((d):d is string => Boolean(d));
    try {
      const [items,events,tasks] = await Promise.all([api.agendaList(days[0],days.at(-1)!),api.eventList(days[0],days.at(-1)!),api.taskList()]);
      if (version===generation.current && currentMonth.current===month) {setData({month,items,events,tasks});setError(null);}
    } catch(e) {if(version===generation.current && currentMonth.current===month) {setError(asIpcError(e).message);setData({month:'',items:[],events:[],tasks:[]});}}
    finally {if(version===generation.current) setLoading(false);}
  },[month]);
  useEffect(() => {
    void reload();
    const focus=()=>{void reload();};
    window.addEventListener('focus',focus);
    window.addEventListener('rei-assistant-changed',focus);
    return ()=>{invalidate();window.removeEventListener('focus',focus);window.removeEventListener('rei-assistant-changed',focus);};
  },[reload,invalidate]);
  useEffect(()=>{
    if(loading || data.month!==month)return;
    let active=true;let timer:ReturnType<typeof setTimeout>|undefined;
    const signatures=Object.fromEntries(monthDays(month).filter((day):day is string=>Boolean(day)).map(day=>[day,daySignature(data,day)]));
    setDayLabels(previous=>Object.fromEntries(Object.entries(previous).filter(([day,cached])=>cached.signature===signatures[day])));
    setLabelState('waiting');
    const read=async()=>{
      try {const result=await api.calendarDayLabels(month);if(!active)return;
        const allowed=['School','Work','Holiday','Personal','Mixed','Free'];
        const safe=Object.fromEntries(Object.entries(result.labels).filter(([day,label])=>signatures[day]!==undefined && allowed.includes(label)).map(([day,label])=>[day,{label,signature:signatures[day]}]));
        setDayLabels(safe);setLabelState(result.state);
        if(['busy','changed'].includes(result.state))timer=setTimeout(()=>void read(),30000);
        else if(result.state==='unavailable')timer=setTimeout(()=>void read(),300000);
      } catch {if(active){setLabelState('unavailable');timer=setTimeout(()=>void read(),300000);}}
    };void read();return()=>{active=false;if(timer)clearTimeout(timer);};
  },[data,month,loading]);
  function changeMonth(offset: number) {
    const date=new Date(`${month}-01T12:00:00`);
    const next=dateKey(new Date(date.getFullYear(),date.getMonth()+offset,1)).slice(0,7);
    setMonth(next);setSelected(`${next}-01`);
  }
  async function mutate(action:()=>Promise<unknown>, message?:string) {
    if(busy) return false;
    setBusy(true);setError(null);setSaved(null);
    try {await action();await reload();if(message)setSaved(message);return true;}
    catch(e) {setError(asIpcError(e).message);return false;}
    finally {setBusy(false);}
  }
  async function createEvent(e:FormEvent) {
    e.preventDefault();
    const date=new Date(`${eventDate}T12:00:00`);
    const nextDate=dateKey(new Date(date.getFullYear(),date.getMonth(),date.getDate()+1));
    const ok=await mutate(async()=>{
      await api.eventCreate({title:title.trim(),all_day:true,start_local:eventDate,end_local:nextDate,tz:Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC'});
      setTitle('');setAdding(false);
    },'Event saved.');
    if(ok){setSelected(eventDate);setMonth(eventDate.slice(0,7));}
  }
  const visible=data.month===month && !loading;
  const rows=visible ? entriesForDay(data.items,data.events,selected,true) : [];
  return <>
    <div className="month-toolbar">
      <div className="month-title-group"><p className="eyebrow">YOUR SCHEDULE</p><div className="month-navigation"><h2>{new Date(`${month}-01T12:00:00`).toLocaleDateString('en-GB',{month:'long',year:'numeric'})}</h2><div className="month-controls"><button aria-label="Previous month" disabled={busy} onClick={()=>changeMonth(-1)}>←</button><button disabled={busy} onClick={()=>{setMonth(today.slice(0,7));setSelected(today);}}>Today</button><button aria-label="Next month" disabled={busy} onClick={()=>changeMonth(1)}>→</button></div></div></div>
      <button className="primary" disabled={busy} onClick={()=>{setEventDate(selected);setAdding(!adding);}}>+ Add event</button>
    </div>
    <p className="muted">A month at a glance. Choose a date to see everything planned for that day.</p>
    {error && <p role="alert">{error} <button onClick={()=>void reload()}>Try again</button></p>}
    {saved && <p role="status" className="success">{saved}</p>}
    {adding && <form className="card calendar-event-form" aria-label="New calendar event" onSubmit={e=>void createEvent(e)}><fieldset disabled={busy}>
      <label>Event title<input required maxLength={500} value={title} onChange={e=>setTitle(e.target.value)} /></label>
      <label>Date<input required type="date" value={eventDate} onChange={e=>setEventDate(e.target.value)} /></label>
      <p className="muted">No set time. Shown as an all-day event.</p>
      <button type="button" onClick={()=>setAdding(false)}>Cancel</button><button type="submit" className="primary" disabled={!title.trim()}>Save event</button>
    </fieldset></form>}
    {loading && <p role="status">Loading month…</p>}
    <div className="month-scroll" tabIndex={0} aria-label="Scrollable month calendar"><section className="month-grid" aria-label="Month calendar" aria-busy={loading}>
      {weekdays.map(d=><div className="month-weekday" key={d.code}><abbr title={d.name}>{d.short}</abbr></div>)}
      {monthDays(month).map((day,index)=>{
        if(!day)return <div key={`blank-${index}`} className="month-blank" aria-hidden="true" />;
        const entries=visible ? entriesForDay(data.items,data.events,day) : [];
        const overview=visible ? dayOverview(entries) : error ? 'Schedule unavailable' : 'Loading';
        const cached=dayLabels[day];
        const label=cached?.signature===daySignature(data,day,entries)?cached.label:undefined;
        const theme=!entries.length?'Free':label && label!=='Free'?label:'Planned';
        // The short theme never hides the underlying commitments.
        const headlines=[...entries].sort((a,b)=>Number(b.kind==='event')-Number(a.kind==='event')).slice(0,2);
        return <button key={day} className={`month-day ${day===selected?'selected':''} ${day===today?'is-today':''}`} aria-label={`${longDay(day)}: ${overview}`} aria-pressed={day===selected} aria-current={day===today?'date':undefined} onClick={()=>{setSelected(day);}}>
          <time dateTime={day}>{Number(day.slice(-2))}</time><span className={`day-theme day-theme-${theme.toLowerCase()}`}>{visible ? theme : '—'}</span>
          {headlines.map(i=><span className={`month-entry month-entry-${i.kind}`} key={`${i.kind}-${i.id}-${i.original_local ?? ''}`}>{i.title}</span>)}
          {entries.length>2 && <small className="muted">+{entries.length-2} more</small>}
        </button>;
      })}
    </section></div>
<p className="calendar-scope muted">{labelState==='ready'?'Day labels by AI · ':labelState==='facts_only'?'':labelState==='off'?'Automatic assistance is paused or off · ':labelState==='busy'||labelState==='waiting'?'AI is updating changed days · ':'AI labels are unavailable right now · '}Free means no saved activities. Select a date for all events and deadlines. Times keep their saved time zones.</p>
    <section className="card selected-day" aria-label="Selected day schedule"><div className="section-head"><h2>{longDay(selected)}</h2><span className="muted">{visible ? dayOverview(rows) : 'Schedule unavailable'}</span></div>
      {visible && !rows.length && <p className="empty-state">No scheduled events. Plan a task or add an event when you are ready.</p>}
      {visible && rows.length>0 && <ul className="rows">{rows.map(i=>{
        const ev=data.events.find(e=>i.kind==='event' && e.id===i.id);
        const task=data.tasks.find(t=>t.id===i.id && i.kind!=='event');
        const zone=ev?.tz ?? (i.kind==='occurrence' ? task?.recurrence?.tz : i.kind==='plan' ? task?.plan_tz : task?.due_tz);
        return <li key={`${i.kind}-${i.id}-${i.original_local ?? ''}`}>
          <div className="row-main"><small className="muted">{kindLabel(i.kind)}{zone?` · ${zone}`:''}{i.state==='Done'||i.state==='done'?' · Done':i.state==='Skipped'?' · Skipped':''}</small><strong>{i.title}</strong><span>{i.all_day?'All day':i.local_start.slice(0,10)!==selected?`Continues from ${i.local_start.replace('T',' ')}`:i.local_start.slice(11,16)}{ev && !ev.all_day?` → ${ev.end_local.slice(0,10)===selected?ev.end_local.slice(11,16):ev.end_local.replace('T',' ')}`:''}</span>{i.location && <span className="meeting-location">Room {i.location}</span>}{i.conflict && <span className="conflict-note">Overlaps another item</span>}</div>
          {task && !i.read_only && <button disabled={busy} aria-label={`${task.recurrence?'Edit schedule':'Edit task'}: ${task.title}`} onClick={()=>setEditor({mode:'edit',task})}>{task.recurrence?'Edit schedule':'Edit task'}</button>}
          {i.kind==='occurrence' && i.original_local && <OccurrenceControls item={i} onOpenSchedule={task?.recurrence && !i.read_only?()=>setEditor({mode:'edit',task}):undefined} onEdit={async(item,action)=>{await mutate(()=>api.occurrenceEdit({task_id:item.id,expected_revision:item.task_revision!,original_local:item.original_local!,scope:'this',action}));}} />}
          {ev && (i.read_only?<small className="muted">Read-only</small>:<DeleteAction title={ev.title} kind="event" disabled={busy} onConfirm={()=>mutate(()=>api.eventDelete(ev.id,ev.revision),'Event deleted.')}/>)}
        </li>;
      })}</ul>}
    </section>
    {editor && <TaskEditor editor={editor} onClose={()=>setEditor(null)} onSaved={reload} />}
  </>;
}
