import { dateKey, dayOverview, entriesForDay, monthDays, recurrenceLabel } from '../../../src/schedule';
import type { AgendaItem, CalEvent } from '../../../src/ipc';

const event=(start:string,end:string,all_day=false):CalEvent=>({id:'event',title:'Synthetic lecture',revision:1,all_day,start_local:start,end_local:end,start_utc:null,end_utc:null,tz:'Europe/Berlin',provider:null,read_only:false});
const item=(extra:Partial<AgendaItem>={}):AgendaItem=>({kind:'task',id:'task',title:'Synthetic sheet',local_start:'2026-10-19',all_day:true,state:'todo',original_local:null,task_revision:1,recurring:false,read_only:false,conflict:false,...extra});

it('lays out a Monday-first whole month including leap day and full trailing weeks',()=>{
  const days=monthDays('2028-02');
  expect(days[0]).toBeNull();expect(days[1]).toBe('2028-02-01');
  expect(days.filter(Boolean)).toHaveLength(29);expect(days).toContain('2028-02-29');
  expect(days.length%7).toBe(0);expect(dateKey(new Date(2026,11,31))).toBe('2026-12-31');
});
it('counts events and deadlines separately without hiding either commitment',()=>{
  const ev=event('2026-10-19T09:00:00','2026-10-19T11:00:00');
  const rows=entriesForDay([item(),item({kind:'event',id:'event',title:ev.title,all_day:false,local_start:ev.start_local})],[ev],'2026-10-19');
  expect(rows).toHaveLength(2);expect(dayOverview(rows)).toBe('1 event · 1 deadline');
  expect(dayOverview([])).toBe('No scheduled events');
});
it('shows overnight and all-day events on covered dates with exclusive endings',()=>{
  const overnight=event('2026-10-19T22:00:00','2026-10-20T02:00:00');
  expect(entriesForDay([],[overnight],'2026-10-20')).toHaveLength(1);
  expect(entriesForDay([],[event('2026-10-19T22:00:00','2026-10-20T00:00:00')],'2026-10-20')).toHaveLength(0);
  const allDay=event('2026-10-19','2026-10-21',true);
  expect(entriesForDay([],[allDay],'2026-10-20')).toHaveLength(1);
  expect(entriesForDay([],[allDay],'2026-10-21')).toHaveLength(0);
});
it('excludes skipped/cancelled work from month summaries but allows skipped detail for Undo',()=>{
  const rows=[item({kind:'occurrence',state:'Skipped'}),item({id:'cancelled',state:'cancelled'})];
  expect(entriesForDay(rows,[],'2026-10-19')).toHaveLength(0);
  const detail=entriesForDay(rows,[],'2026-10-19',true);
  expect(detail).toHaveLength(1);expect(dayOverview(detail)).toBe('No scheduled events');
});
it('labels selected weekdays and their individual saved times, including legacy series',()=>{
  const rec={rrule:'FREQ=WEEKLY;BYDAY=MO,TH',tz:'Europe/Berlin',dtstart_local:'2026-10-19T09:00:00',weekday_times:{MO:'09:00',TH:'14:00'}};
  expect(recurrenceLabel(rec)).toBe('Repeats every week · Mon 09:00 · Thu 14:00 (Europe/Berlin)');
  expect(recurrenceLabel({...rec,weekday_times:undefined})).toContain('Thu 09:00');
});
