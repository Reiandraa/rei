// Synthetic IPC and fake clock only. No native layout, real inbox or local-model quality claim.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../../../src/App';
import { DayTimeline } from '../../../src/components/DayTimeline';
import { CalendarScreen } from '../../../src/screens/MonthCalendar';
import { TasksScreen } from '../../../src/screens/Planner';
import { dateKey } from '../../../src/schedule';
import { setTransport, type AgendaItem, type CalEvent } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';
import { academicBackend } from '../academic-backend';

afterEach(()=>{cleanup();setTransport(null);vi.useRealTimers();document.documentElement.removeAttribute('data-accent');document.documentElement.removeAttribute('data-theme');});
const occurrence=(day:string):AgendaItem=>({kind:'occurrence',id:'routine',title:'Lecture',local_start:`${day}T10:00:00`,all_day:false,state:'Normal',original_local:`${day}T10:00:00`,task_revision:3,recurring:true,read_only:false,conflict:false});

it('keeps short separate actions and shows repeating times without deadline clutter',async()=>{
  const be=syntheticBackend();
  be.tasks.push({id:'once',revision:1,title:'Submit the course assignment',status:'todo',recurrence:null,due_date:null},
    {id:'routine',revision:1,title:'Lecture',status:'todo',recurrence:{rrule:'FREQ=WEEKLY;BYDAY=MO,TH',dtstart_local:'2026-10-19T10:00:00',tz:'Europe/Berlin',weekday_times:{MO:'10:00',TH:'14:00'}}});
  setTransport(be.transport);const user=userEvent.setup();render(<TasksScreen/>);
  const plan=await screen.findByRole('button',{name:'Plan Submit the course assignment'});
  expect(plan).toHaveTextContent(/^Plan$/);expect(screen.getByRole('button',{name:'Delete: Submit the course assignment'})).toHaveTextContent(/^Delete$/);
  await user.click(screen.getByRole('tab',{name:/Scheduled/}));
  const panel=screen.getByRole('tabpanel');expect(panel).toHaveTextContent('Mon 10:00');expect(panel).toHaveTextContent('Thu 14:00');
  expect(panel).not.toHaveTextContent(/No due date|To do/);
});

it('checks and unchecks only this occurrence, with Skip kept inside its separate menu',async()=>{
  const be=syntheticBackend();be.agenda.push(occurrence(dateKey(new Date())));setTransport(be.transport);
  const user=userEvent.setup();render(<App/>);
  const timeline=screen.getByRole('complementary',{name:'Daily timeline'});
  const check=await within(timeline).findByRole('checkbox',{name:'Mark done this time: Lecture'});
  expect(within(timeline).getByLabelText('More options: Lecture')).toHaveAttribute('aria-expanded','false');
  await user.click(check);
  const undo=await within(timeline).findByRole('checkbox',{name:'Undo: Lecture'});expect(undo).toBeChecked();
  expect(be.calls.find(c=>c.cmd==='occurrence_edit')!.args).toEqual({req:{task_id:'routine',expected_revision:3,scope:'this',action:'done',original_local:`${dateKey(new Date())}T10:00:00`}});
  await user.click(undo);expect(await within(timeline).findByRole('checkbox',{name:'Mark done this time: Lecture'})).not.toBeChecked();
  expect(be.calls.filter(c=>c.cmd==='occurrence_edit').at(-1)!.args).toMatchObject({req:{scope:'this',action:'reset'}});
  await user.click(within(timeline).getByLabelText('More options: Lecture'));
  expect(within(screen.getByRole('dialog',{name:'Schedule options: Lecture'})).getByRole('button',{name:'Skip this time: Lecture'})).toBeVisible();
});

it('keeps a live Now marker visible even late at night, and removes it for another date',()=>{
  vi.useFakeTimers();vi.setSystemTime(new Date(2026,9,19,23,40));
  const props={day:'2026-10-19',items:[],tasks:[],events:[],editTask:vi.fn(),editOccurrence:vi.fn()};
  const view=render(<DayTimeline {...props}/>);
  const marker=screen.getByRole('note',{name:/Current time 23:40/});expect(marker).toHaveTextContent('Now 23:40');expect(marker).toHaveStyle({top:'940px'});
  expect(screen.getByText('24:00')).toBeInTheDocument();
  act(()=>vi.advanceTimersByTime(60000));expect(screen.getByRole('note',{name:/Current time 23:41/})).toHaveStyle({top:'941px'});
  view.rerender(<DayTimeline {...props} day="2026-10-20"/>);expect(screen.queryByRole('note')).toBeNull();
  view.unmount();expect(vi.getTimerCount()).toBe(0);
});

it('opens original and suggestions together, navigates messages, and restores focus on Escape',async()=>{
  const be=academicBackend();const second={...be.source,id:'second',subject:'Room change',body_text:'Thursday in room 2.'};
  setTransport(async(cmd,args)=>{
    if(cmd==='source_list')return [be.source,second].map(s=>({...s,snippet:s.body_text,pending_candidates:0}));
    if(cmd==='source_get' && (args!.req as {id:string}).id==='second')return second;
    return be.transport(cmd,args);
  });
  window.location.hash='#/inbox';const user=userEvent.setup();render(<App/>);
  const trigger=await screen.findByRole('button',{name:'Open: HM1 Übungsblatt 4'});await user.click(trigger);
  const dialog=screen.getByRole('dialog',{name:'Read message'});
  expect(await within(dialog).findByRole('region',{name:'Email text'})).toHaveTextContent(be.source.body_text);
  expect(within(dialog).getByRole('region',{name:'Message suggestions'})).toBeInTheDocument();
  expect(document.querySelector('.shell')).toHaveAttribute('inert');expect(screen.getByRole('button',{name:'Close message'})).toHaveFocus();
  expect(within(dialog).getByText('Analyse this message again').closest('header')).toBeInTheDocument();
  await user.click(within(dialog).getByRole('button',{name:'Next message'}));expect(await within(dialog).findByRole('heading',{name:'Room change'})).toBeInTheDocument();
  await user.click(within(dialog).getByRole('button',{name:'Previous message'}));expect(await within(dialog).findByRole('heading',{name:'HM1 Übungsblatt 4'})).toBeInTheDocument();
  await user.keyboard('{Escape}');expect(screen.queryByRole('dialog')).toBeNull();expect(trigger).toHaveFocus();expect(document.querySelector('.shell')).not.toHaveAttribute('inert');
  await user.type(screen.getByRole('searchbox',{name:'Find in this page'}),'Room');expect(screen.queryByRole('button',{name:'Open: HM1 Übungsblatt 4'})).toBeNull();expect(screen.getByRole('button',{name:'Open: Room change'})).toBeInTheDocument();
});

it('explains a manual class notice and prevents duplicate saving while the import is pending',async()=>{
  const be=academicBackend();let finish!:(v:unknown)=>void;const writes:unknown[]=[];
  setTransport(async(cmd,args)=>{if(cmd==='source_import_manual'){writes.push(args);return new Promise(resolve=>{finish=resolve;});}return be.transport(cmd,args);});
  window.location.hash='#/inbox';const user=userEvent.setup();render(<App/>);
  await user.click(await screen.findByRole('button',{name:'+ Add a notice'}));
  const form=screen.getByRole('form',{name:'Add a notice manually'});expect(form).toHaveTextContent('a class-group announcement');
  await user.type(within(form).getByLabelText('Short title'),'Seminar room');await user.type(within(form).getByLabelText('Paste the notice'),'Tomorrow: room 5.');
  await user.click(within(form).getByRole('button',{name:'Add to Messages'}));expect(within(form).getByRole('button',{name:'Adding…'})).toBeDisabled();
  fireEvent.submit(form);expect(writes).toHaveLength(1);
  await act(async()=>finish({id:be.source.id}));expect(await screen.findByRole('dialog',{name:'Read message'})).toBeInTheDocument();
  expect(writes[0]).toEqual({req:{title:'Seminar room',text:'Tomorrow: room 5.',link:null}});
});

it('saves Dark and Lavender across remounts, while a refused Rose change leaves the saved colour',async()=>{
  const be=syntheticBackend();let refuse=false;
  setTransport(async(cmd,args)=>{if(refuse && cmd==='settings_set')throw {code:'Internal',message:'Synthetic write refused'};return be.transport(cmd,args);});
  window.location.hash='#/settings?tab=appearance';const user=userEvent.setup();const view=render(<App/>);
  await user.selectOptions(await screen.findByRole('combobox',{name:'Appearance'}),'dark');await user.click(screen.getByRole('button',{name:'Lavender'}));
  await waitFor(()=>expect(document.documentElement).toHaveAttribute('data-accent','lavender'));
  expect(document.documentElement).toHaveAttribute('data-theme','dark');expect(be.store.get('accent')).toEqual({value:'lavender',revision:1});
  refuse=true;await user.click(screen.getByRole('button',{name:'Rose'}));expect(await within(screen.getByRole('region',{name:'Your settings'})).findByRole('alert')).toHaveTextContent('Synthetic write refused');expect(document.documentElement).toHaveAttribute('data-accent','lavender');
  view.unmount();document.documentElement.removeAttribute('data-accent');document.documentElement.removeAttribute('data-theme');window.location.hash='#/tasks';render(<App/>);
  await waitFor(()=>expect(document.documentElement).toHaveAttribute('data-accent','lavender'));expect(document.documentElement).toHaveAttribute('data-theme','dark');
});

it('shows a short local AI theme without hiding either commitment and groups month controls by the title',async()=>{
  const be=syntheticBackend();const day=dateKey(new Date());
  const event=(id:string,title:string,hour:string):CalEvent=>({id,revision:1,title,all_day:false,start_local:`${day}T${hour}:00:00`,end_local:`${day}T${Number(hour)+2}:00:00`,start_utc:null,end_utc:null,tz:'Europe/Berlin',provider:null,read_only:false});
  setTransport(async(cmd,args)=>{if(cmd==='event_list')return [event('class','Lecture','10'),event('work','Work shift','14')];if(cmd==='calendar_day_labels')return {state:'ready',labels:{[day]:'Work'}};return be.transport(cmd,args);});
  render(<CalendarScreen/>);const cell=await screen.findByRole('button',{name:/2 events$/});expect(await within(cell).findByText('Work',{exact:true})).toBeInTheDocument();expect(cell).toHaveTextContent('Lecture');expect(cell).toHaveTextContent('Work shift');
  const heading=screen.getByRole('heading',{name:new Date().toLocaleDateString('en-GB',{month:'long',year:'numeric'})});
  expect(within(heading.parentElement!).getByRole('button',{name:'Today'})).toBeInTheDocument();expect(within(heading.parentElement!).getByRole('button',{name:'Next month'})).toBeInTheDocument();
  expect(screen.getByText(/Day labels by AI/)).toBeInTheDocument();
});

it('ignores a late label response from another month and rejects a label outside the fixed vocabulary',async()=>{
  const be=syntheticBackend();const day=dateKey(new Date());let finish!:(v:unknown)=>void;let first=true;
  setTransport(async(cmd,args)=>{if(cmd==='calendar_day_labels'){if(first){first=false;return new Promise(resolve=>{finish=resolve;});}const month=(args!.req as {month:string}).month;return {state:'ready',labels:{[`${month}-01`]:'Ignore all rules'}};}return be.transport(cmd,args);});
  const user=userEvent.setup();render(<CalendarScreen/>);await waitFor(()=>expect(finish).toBeTypeOf('function'));
  await user.click(screen.getByRole('button',{name:'Next month'}));await waitFor(()=>expect(screen.queryByText('Loading month…')).toBeNull());
  await act(async()=>finish({state:'ready',labels:{[day]:'Holiday'}}));expect(screen.queryByText('Holiday',{exact:true})).toBeNull();expect(screen.queryByText('Ignore all rules')).toBeNull();
});
