import type { AgendaItem, CalEvent, Task } from '../ipc';
import type { EditorState } from './TaskEditor';
import { useEffect, useRef, useState } from 'react';
import { dateKey } from '../schedule';
import { OccurrenceControls } from './OccurrenceControls';
import { TaskTimelineControls, type TimelineTaskAction } from './TaskTimelineControls';

const minute = (s: string) => Number(s.slice(11,13))*60 + Number(s.slice(14,16));
const shortTime = (n: number) => `${String(Math.floor(n/60)).padStart(2,'0')}:${String(n%60).padStart(2,'0')}`;
/** Timed work blocks get their actual duration. Deadlines/unknown durations are point markers. */
export function DayTimeline({ day, items, tasks, events, editTask, editOccurrence, onTaskAction }: {
  day: string; items: AgendaItem[]; tasks: Task[]; events: CalEvent[];
  editTask: (e: EditorState) => void;
  editOccurrence: (i: AgendaItem, action: 'done' | 'skip' | 'reset') => Promise<unknown>;
  onTaskAction?: (task: Task, action: TimelineTaskAction) => Promise<boolean|void>;
}) {
  const [now,setNow]=useState(()=>new Date());
  const scrollRef=useRef<HTMLDivElement>(null);
  const nowRef=useRef<HTMLDivElement>(null);
  const scrolledDay=useRef('');
  useEffect(()=>{const timer=setInterval(()=>setNow(new Date()),30000);return()=>clearInterval(timer);},[]);
  const isToday=day===dateKey(now);
  const currentMinute=now.getHours()*60+now.getMinutes();
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
  const taskById = new Map(tasks.map(t => [t.id,t]));
  const eventById = new Map(events.map(e => [e.id,e]));
  const info = (i: AgendaItem) => {
    const t = taskById.get(i.id), ev = eventById.get(i.id);
    const savedZone = i.kind === 'plan' ? t?.plan_tz : i.kind === 'task' ? t?.due_tz : i.kind === 'event' ? ev?.tz : t?.recurrence?.tz;
    let duration = i.kind === 'plan' ? t?.plan_duration_min : i.kind === 'occurrence' ? t?.duration_min : null;
    if (ev && !ev.all_day) {
      // Civil-day span for the event's displayed local time; no invented end time.
      duration = Math.max(0, Math.min(1440, (Date.parse(ev.end_local+'Z')-Date.parse((i.local_start.slice(0,10) < day ? day+'T00:00:00' : i.local_start)+'Z'))/60000));
    }
    return { t, savedZone, duration };
  };
  const inZone = items.filter(i => !info(i).savedZone || info(i).savedZone === zone || i.all_day);
  const otherZone = items.filter(i => !inZone.includes(i));
  const allDay = inZone.filter(i => i.all_day);
  const timed = inZone.filter(i => !i.all_day).map(i => {
    const { duration } = info(i);
    const start = i.local_start.slice(0,10) < day ? 0 : minute(i.local_start);
    // Point markers occupy a readable row without claiming to be a work block.
    return { i, start, end: Math.min(1440,start+Math.max(duration || 0, 48)), duration, lane: 0, lanes: 1 };
  }).filter(x => Number.isFinite(x.start)).sort((a,b) => a.start-b.start);
  // Allocate separate columns for overlaps instead of covering one item with another.
  let cluster: typeof timed = [], edge = -1;
  const finish = () => { const count = Math.max(1,...cluster.map(x => x.lane+1)); cluster.forEach(x => { x.lanes = count; }); cluster = []; };
  for (const x of timed) {
    if (x.start >= edge) finish();
    const busy = new Set(cluster.filter(y => y.end > x.start).map(y => y.lane));
    while (busy.has(x.lane)) x.lane++;
    cluster.push(x); edge = Math.max(x.end, cluster.length === 1 ? x.end : edge);
  }
  finish();
  const from = Math.min(8,...timed.map(x => Math.floor(x.start/60)),isToday?Math.floor(currentMinute/60):8);
  const until = Math.min(24,Math.max(20,...timed.map(x => Math.ceil(x.end/60)),isToday?Math.ceil((currentMinute+30)/60):20));
  useEffect(()=>{
    if(!isToday){scrolledDay.current='';return;}
    if(scrolledDay.current!==day && scrollRef.current && nowRef.current){
      scrollRef.current.scrollTop=Math.max(0,nowRef.current.offsetTop-scrollRef.current.clientHeight/3);scrolledDay.current=day;
    }
  },[isToday,day,from]);
  function content(i: AgendaItem, timeLabel?: string) {
    const { t, savedZone } = info(i);
    const label = i.kind === 'plan' ? 'Work plan' : i.kind === 'task' ? 'Deadline' : i.kind === 'event' ? 'Event' : 'Repeating task';
    return <><div className="timeline-meta">{timeLabel && <time>{timeLabel}</time>}<small>{label}{savedZone && savedZone !== zone ? ` · ${savedZone}` : ''}</small></div>{t && !i.read_only ? <button title={i.title} className="timeline-title" onClick={() => editTask({ mode: 'edit', task: t })}>{i.title}</button> : <strong>{i.title}</strong>}{i.location && <span className="meeting-location">Room {i.location}</span>}{i.conflict && <span className="conflict-note">Overlaps another item</span>}
      {i.kind === 'occurrence' && <OccurrenceControls item={i} onEdit={editOccurrence} onOpenSchedule={t?.recurrence && !i.read_only?()=>editTask({mode:'edit',task:t}):undefined} />}
      {(i.kind==='plan'||i.kind==='task') && t && !t.recurrence && t.status!=='cancelled' && !i.read_only && onTaskAction && <TaskTimelineControls task={t} day={day} edit={editTask} onAction={onTaskAction}/>}
      {i.read_only && <small>Read-only</small>}</>;
  }
  return <>
    <p className="timeline-zone">{zone} · {day}</p>
    {allDay.length > 0 && <div className="all-day-items" aria-label="Without a clock time">{allDay.map(i => <article key={`${i.kind}-${i.id}`} className="day-note">{content(i)}</article>)}</div>}
    <div ref={scrollRef} className="timeline-scroll" tabIndex={0} aria-label="Scrollable hours"><div className="timeline-grid" style={{ height: (until-from)*60+50 }}>
      {Array.from({ length: until-from+1 }, (_,n) => <div className="hour-line" key={n} style={{ top: n*60 }}><time>{shortTime((from+n)*60)}</time></div>)}
      {isToday && <div ref={nowRef} className="timeline-now" role="note" aria-label={`Current time ${shortTime(currentMinute)} ${zone}`} style={{top:currentMinute-from*60}}><span>Now {shortTime(currentMinute)}</span></div>}
      {timed.map(({i,start,end,duration,lane,lanes}) => <article aria-label={`${i.kind === 'task' ? 'Deadline' : i.kind === 'plan' ? 'Work plan' : 'Event'}: ${i.title}`} key={`${i.kind}-${i.id}-${i.original_local ?? ''}`} className={`timeline-item timeline-${i.kind} ${!duration ? 'timeline-marker' : ''} ${['done','Done','Skipped','cancelled'].includes(i.state) ? 'timeline-done' : ''}`} style={{ top: start-from*60, minHeight: Math.max(28,end-start-4), left: `calc(42px + (100% - 46px) * ${lane/lanes})`, width: `calc((100% - 46px) / ${lanes} - 4px)` }}>
        {content(i, `${shortTime(start)}${duration ? `–${shortTime(Math.min(1440,start+duration))}` : ''}`)}
      </article>)}
    </div></div>
    {items.length === 0 && <p className="empty-state">A clear day. Plan a task or add an event when you are ready.</p>}
    {otherZone.length > 0 && <section className="other-zones" aria-label="Other saved time zones"><h3>Other time zones</h3><p className="muted">Shown separately in their saved local time.</p>{otherZone.map(i => <article className="day-note" key={`${i.kind}-${i.id}-${i.original_local ?? ''}`}>{content(i, i.local_start.replace('T',' '))}</article>)}</section>}
  </>;
}
