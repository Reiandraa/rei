import type { AgendaItem, CalEvent, RecurrenceIn } from './ipc';

export const weekdays = [
  { code: 'MO', short: 'Mon', name: 'Monday' }, { code: 'TU', short: 'Tue', name: 'Tuesday' },
  { code: 'WE', short: 'Wed', name: 'Wednesday' }, { code: 'TH', short: 'Thu', name: 'Thursday' },
  { code: 'FR', short: 'Fri', name: 'Friday' }, { code: 'SA', short: 'Sat', name: 'Saturday' },
  { code: 'SU', short: 'Sun', name: 'Sunday' },
] as const;
export const dateKey = (d: Date) => `${d.getFullYear()}-${String(d.getMonth()+1).padStart(2,'0')}-${String(d.getDate()).padStart(2,'0')}`;
export function monthDays(month: string): (string | null)[] {
  const first = new Date(`${month}-01T12:00:00`);
  const last = new Date(first.getFullYear(),first.getMonth()+1,0).getDate();
  const days: (string | null)[] = Array.from({length:(first.getDay()+6)%7}, () => null);
  for (let n=1;n<=last;n++) days.push(`${month}-${String(n).padStart(2,'0')}`);
  while (days.length%7) days.push(null);
  return days;
}
export function recurrenceLabel(rec: RecurrenceIn): string {
  const rule = Object.fromEntries(rec.rrule.replace(/^RRULE:/,'').split(';').map(s => s.split('=')));
  if (rule.FREQ === 'WEEKLY') {
    const days = rule.BYDAY?.split(',') ?? [weekdays[(new Date(`${rec.dtstart_local.slice(0,10)}T12:00:00`).getDay()+6)%7].code];
    const times = weekdays.filter(d => days.includes(d.code)).map(d => `${d.short} ${rec.weekday_times?.[d.code] ?? rec.dtstart_local.slice(11,16)}`.trim());
    return `Repeats every ${rule.INTERVAL && rule.INTERVAL !== '1' ? `${rule.INTERVAL} weeks` : 'week'} · ${times.join(' · ')} (${rec.tz})`;
  }
  return `Repeats ${rule.FREQ === 'DAILY' ? 'every day' : 'every month'} · ${rec.dtstart_local.slice(11,16)} (${rec.tz})`;
}

/** Uses stored facts only. Event end is exclusive, including all-day and midnight endings. */
export function entriesForDay(items: AgendaItem[], events: CalEvent[], day: string, includeSkipped = false): AgendaItem[] {
  const active = (i: AgendaItem) => i.state !== 'cancelled' && (includeSkipped || i.state !== 'Skipped');
  const rows = items.filter(i => i.kind !== 'event' && i.local_start.slice(0,10) === day && active(i));
  for (const ev of events) {
    const start = ev.start_local.slice(0,10), end = ev.end_local.slice(0,10);
    const endsAfterDayStart = end > day || (end === day && !ev.all_day && ev.end_local.slice(11,19) > '00:00:00');
    if (start <= day && endsAfterDayStart) {
      const existing = items.find(i => i.kind === 'event' && i.id === ev.id);
      rows.push(existing ? {...existing,read_only:existing.read_only || ev.read_only || Boolean(ev.provider)} : {kind:'event',id:ev.id,title:ev.title,local_start:ev.start_local,all_day:ev.all_day,state:'Normal',original_local:null,task_revision:null,recurring:false,read_only:ev.read_only || Boolean(ev.provider),conflict:false});
    }
  }
  // An agenda-only event can still be shown if an event response lacks its metadata.
  for (const i of items.filter(i => i.kind === 'event' && i.local_start.slice(0,10) === day && active(i))) {
    if (!rows.some(r => r.kind === 'event' && r.id === i.id)) rows.push(i);
  }
  return rows.sort((a,b) => a.local_start.localeCompare(b.local_start) || a.title.localeCompare(b.title));
}
export function dayOverview(rows: AgendaItem[]): string {
  rows = rows.filter(i => !['Skipped','cancelled'].includes(i.state));
  if (!rows.length) return 'No scheduled events';
  const groups: [AgendaItem['kind'], string][] = [['event','event'],['occurrence','repeating task'],['plan','work block'],['task','deadline']];
  return groups.flatMap(([kind,label]) => {
    const n=rows.filter(i => i.kind === kind).length;
    return n ? [`${n} ${label}${n===1?'':'s'}`] : [];
  }).join(' · ');
}
