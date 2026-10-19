// Synthetic IPC + jsdom only; actual recurrence persistence is tested in Rust/SQLite.
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { setTransport, type AgendaItem, type CalEvent, type CommandName, type Transport } from '../../../src/ipc';
import { TaskEditor } from '../../../src/components/TaskEditor';
import { CalendarScreen } from '../../../src/screens/MonthCalendar';
import { dateKey } from '../../../src/schedule';
import { syntheticBackend } from '../fake-backend';

afterEach(()=>setTransport(null));
const day=dateKey(new Date());
const ev=(extra:Partial<CalEvent>={}):CalEvent=>({id:'lecture',revision:1,title:'Lecture: algorithms',all_day:false,start_local:`${day}T09:00:00`,end_local:`${day}T11:00:00`,start_utc:null,end_utc:null,tz:'Europe/Berlin',provider:null,read_only:false,...extra});
function calendarBackend(events:CalEvent[],items:AgendaItem[]=[]) {
  const base=syntheticBackend();
  const calls:{cmd:CommandName;args?:Record<string,unknown>}[]=[];
  const transport:Transport=async(cmd,args)=>{
    calls.push({cmd,args});
    if(cmd==='event_list')return events.map(e=>({...e}));
    if(cmd==='agenda_list')return items.map(i=>({...i}));
    if(cmd==='event_delete'){const id=(args!.req as {id:string}).id;events.splice(events.findIndex(e=>e.id===id),1);return null;}
    if(cmd==='event_create'){const req=args!.req as CalEvent;events.push(ev({...req,id:'created'}));return events.at(-1);}
    return base.transport(cmd,args);
  };
  return {...base,calls,transport};
}

it('saves Monday and Thursday at different times in one recurring task, not multiple writes',async()=>{
  const be=syntheticBackend();setTransport(be.transport);
  const user=userEvent.setup();const close=vi.fn();
  render(<TaskEditor editor={{mode:'create'}} onClose={close} onSaved={async()=>{}} />);
  await user.type(screen.getByLabelText('What needs to be done?'),'Study routine');
  await user.click(screen.getByText('More options'));
  await user.selectOptions(screen.getByLabelText('Repeat'),'WEEKLY');
  const picker=screen.getByRole('group',{name:'Repeat on weekdays'});
  for(const button of within(picker).getAllByRole('button'))if(button.getAttribute('aria-pressed')==='true')await user.click(button);
  expect(screen.getByRole('button',{name:'Add task'})).toBeDisabled();
  await user.click(within(picker).getByRole('button',{name:'Monday'}));
  await user.click(within(picker).getByRole('button',{name:'Thursday'}));
  fireEvent.change(screen.getByLabelText('Starting from'),{target:{value:'2026-10-19'}});
  fireEvent.change(screen.getByLabelText('Monday time'),{target:{value:'09:00'}});
  fireEvent.change(screen.getByLabelText('Thursday time'),{target:{value:'14:00'}});
  await user.click(screen.getByRole('button',{name:'Add task'}));
  await waitFor(()=>expect(close).toHaveBeenCalledOnce());
  expect(be.calls.filter(c=>c.cmd==='task_create')).toHaveLength(1);
  expect(be.calls.find(c=>c.cmd==='task_create')!.args).toMatchObject({req:{recurrence:{rrule:'FREQ=WEEKLY;BYDAY=MO,TH',dtstart_local:'2026-10-19T09:00:00',weekday_times:{MO:'09:00',TH:'14:00'}}}});
});

it('keeps both lecture and work visible in the month and all details on the chosen date',async()=>{
  const be=calendarBackend([ev(),ev({id:'shift',title:'Work shift',start_local:`${day}T14:00:00`,end_local:`${day}T18:00:00`})]);setTransport(be.transport);
  render(<CalendarScreen />);
  const cell=await screen.findByRole('button',{name:/2 events$/});
  expect(within(cell).getByText('Lecture: algorithms')).toBeInTheDocument();
  expect(within(cell).getByText('Work shift')).toBeInTheDocument();
  const detail=screen.getByRole('region',{name:'Selected day schedule'});
  expect(within(detail).getByText('Lecture: algorithms')).toBeInTheDocument();
  expect(within(detail).getByText('Work shift')).toBeInTheDocument();
  expect(within(detail).getByText('09:00 → 11:00')).toBeInTheDocument();
  expect(screen.getAllByRole('button',{name:/No scheduled events$/}).length).toBeGreaterThan(20);
  expect(screen.getAllByText(/^Free$/).length).toBeGreaterThan(20);
  expect(screen.getByText(/Free means no saved activities/)).toBeInTheDocument();
  const range=(be.calls.find(c=>c.cmd==='agenda_list')!.args!.req as {from:string;to:string});
  expect(range.from).toBe(`${day.slice(0,7)}-01`);
  expect(range.to).toBe(dateKey(new Date(new Date().getFullYear(),new Date().getMonth()+1,0)));
});

it('navigates across months and does not replace a new month with a late old response',async()=>{
  const be=calendarBackend([]);
  let resolveOld:(items:AgendaItem[])=>void=()=>{};
  let first=true;
  setTransport(async(cmd,args)=>{
    if(cmd==='agenda_list' && first){first=false;return new Promise<AgendaItem[]>(resolve=>{resolveOld=resolve;});}
    return be.transport(cmd,args);
  });
  const user=userEvent.setup();render(<CalendarScreen />);
  await user.click(screen.getByRole('button',{name:'Next month'}));
  await waitFor(()=>expect(screen.queryByText('Loading month…')).toBeNull());
  await act(async()=>resolveOld([{kind:'task',id:'old',title:'Stale month task',local_start:day,all_day:true,state:'todo',original_local:null,task_revision:1,recurring:false,read_only:false,conflict:false}]));
  expect(screen.queryByText('Stale month task')).toBeNull();
  const next=new Date(new Date().getFullYear(),new Date().getMonth()+1,1);
  expect(screen.getByRole('heading',{name:next.toLocaleDateString('en-GB',{month:'long',year:'numeric'})})).toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:'Today'}));
  expect(await screen.findByRole('heading',{name:new Date().toLocaleDateString('en-GB',{month:'long',year:'numeric'})})).toBeInTheDocument();
});

it('shows failed month loading as unavailable, never as an empty or free day',async()=>{
  const be=calendarBackend([]);setTransport(async(cmd,args)=>{if(cmd==='agenda_list')throw {code:'Offline',message:'Synthetic DB read failed'};return be.transport(cmd,args);});
  render(<CalendarScreen />);
  expect(await screen.findByRole('alert')).toHaveTextContent('Synthetic DB read failed');
  expect(screen.queryAllByRole('button',{name:/No scheduled events$/})).toHaveLength(0);
  expect(screen.getAllByRole('button',{name:/Schedule unavailable$/}).length).toBeGreaterThan(27);
});

it('keeps source-owned events read only and requires confirmation for a local deletion',async()=>{
  const be=calendarBackend([ev({provider:'fixture',read_only:true}),ev({id:'local',title:'Local appointment'})]);setTransport(be.transport);
  const user=userEvent.setup();render(<CalendarScreen />);
  const detail=screen.getByRole('region',{name:'Selected day schedule'});
  expect(await within(detail).findByText('Read-only')).toBeInTheDocument();
  expect(within(detail).queryByRole('button',{name:'Delete: Lecture: algorithms'})).toBeNull();
  await user.click(within(detail).getByRole('button',{name:'Delete: Local appointment'}));
  expect(be.calls.filter(c=>c.cmd==='event_delete')).toHaveLength(0);
  await user.click(within(screen.getByRole('dialog',{name:'Delete event: Local appointment'})).getByRole('button',{name:'Confirm delete: Local appointment'}));
  await waitFor(()=>expect(be.calls.filter(c=>c.cmd==='event_delete')).toHaveLength(1));
  expect(be.calls.find(c=>c.cmd==='event_delete')!.args).toEqual({req:{id:'local',expected_revision:1,confirm:true}});
});

it('adds a date-only calendar event without clock fields, including a year-end date',async()=>{
  const be=calendarBackend([]);setTransport(be.transport);
  const user=userEvent.setup();render(<CalendarScreen />);
  await user.click(screen.getByRole('button',{name:'+ Add event'}));
  const form=screen.getByRole('form',{name:'New calendar event'});
  expect(within(form).getByLabelText('Date')).toHaveValue(day);
  expect(form.querySelector('input[type="datetime-local"],input[type="time"]')).toBeNull();
  fireEvent.change(within(form).getByLabelText('Date'),{target:{value:'2026-12-31'}});
  await user.type(screen.getByLabelText('Event title'),'Tutorial');
  await user.click(screen.getByRole('button',{name:'Save event'}));
  expect(await screen.findByText('Event saved.')).toBeInTheDocument();
  expect(be.calls.find(c=>c.cmd==='event_create')!.args).toMatchObject({req:{title:'Tutorial',start_local:'2026-12-31',end_local:'2027-01-01',all_day:true}});
  expect(await within(screen.getByRole('region',{name:'Selected day schedule'})).findByText('All day')).toBeInTheDocument();
});

it('shows a deadline beside a repeating task and lets a skipped occurrence be restored',async()=>{
  const occurrence:AgendaItem={kind:'occurrence',id:'repeat',title:'Study routine',local_start:`${day}T14:00:00`,all_day:false,state:'Normal',original_local:`${day}T14:00:00`,task_revision:4,recurring:true,read_only:false,conflict:false};
  const deadline:AgendaItem={...occurrence,kind:'task',id:'sheet',title:'Submit sheet',local_start:day,all_day:true,state:'todo',recurring:false,original_local:null};
  const be=calendarBackend([],[occurrence,deadline]);
  const calls:unknown[]=[];
  setTransport(async(cmd,args)=>{
    if(cmd==='occurrence_edit'){
      calls.push(args);
      const req=args!.req as {action:string};
      occurrence.state=req.action==='skip'?'Skipped':'Normal';
      occurrence.task_revision!++;
      return {};
    }
    return be.transport(cmd,args);
  });
  const user=userEvent.setup();render(<CalendarScreen />);
  const detail=screen.getByRole('region',{name:'Selected day schedule'});
  expect(await screen.findByRole('button',{name:/1 repeating task · 1 deadline$/})).toBeInTheDocument();
  await user.click(within(detail).getByLabelText('More options: Study routine'));
  await user.click(within(screen.getByRole('dialog',{name:'Schedule options: Study routine'})).getByRole('button',{name:'Skip this time: Study routine'}));
  expect(await within(detail).findByRole('button',{name:'Undo: Study routine'})).toBeInTheDocument();
  expect(calls[0]).toEqual({req:{task_id:'repeat',expected_revision:4,original_local:`${day}T14:00:00`,scope:'this',action:'skip'}});
  expect(screen.getByRole('button',{name:/1 deadline$/})).toBeInTheDocument();
  await user.click(within(detail).getByRole('button',{name:'Undo: Study routine'}));
  expect(await within(detail).findByRole('checkbox',{name:'Mark done this time: Study routine'})).not.toBeChecked();
  expect(calls[1]).toMatchObject({req:{expected_revision:5,action:'reset'}});
});
