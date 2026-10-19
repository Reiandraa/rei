// Synthetic IPC/jsdom, not native layout or real email/model evidence.
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { setTransport } from '../../../src/ipc';
import { TasksScreen } from '../../../src/screens/Planner';
import { DailyReview } from '../../../src/screens/DailyReview';
import { CandidateCard } from '../../../src/screens/Inbox';
import { ScheduleNotice, suggestedScope, suggestedRoom } from '../../../src/components/ScheduleNotice';
import { syntheticBackend } from '../fake-backend';
import { autoBackend, cand } from '../auto-backend';

afterEach(()=>setTransport(null));
const routine={id:'routine',title:'Algorithmen',revision:4,status:'todo',recurrence:{series_id:'series',rrule:'FREQ=WEEKLY;BYDAY=MO,TH',dtstart_local:'2026-10-19T09:00:00',tz:'Europe/Berlin',weekday_times:{MO:'09:00',TH:'10:00'}},due_date:null,due_at_local:null};
const notice=()=>cand(1,{kind:'update',title:'Lecture moved',source_subject:'Algorithmen',excerpt:'Algorithmen findet nur am Donnerstag in Raum B statt.',due_date:'2026-10-22',due_time:null});

it('separates one-time tasks from recurring routines and supports keyboard tab navigation',async()=>{
  const be=syntheticBackend();be.tasks.push({...routine},{...routine,id:'once',title:'Submit sheet',recurrence:null});setTransport(be.transport);
  const user=userEvent.setup();render(<TasksScreen />);
  expect(await screen.findByRole('button',{name:'Edit Submit sheet'})).toBeInTheDocument();
  expect(screen.queryByRole('button',{name:'Edit Algorithmen'})).toBeNull();
  const scheduled=screen.getByRole('tab',{name:'Scheduled 1'});
  await user.click(scheduled);
  expect(screen.getByRole('button',{name:'Edit Algorithmen'})).toBeInTheDocument();
  expect(screen.queryByRole('button',{name:'Edit Submit sheet'})).toBeNull();
  await user.click(screen.getByRole('button',{name:'+ Add routine'}));
  expect(screen.getByLabelText('Repeat')).toHaveValue('WEEKLY');
  expect(screen.getByRole('group',{name:'Repeat on weekdays'})).toBeVisible();
  await user.click(screen.getByRole('button',{name:'Cancel'}));
  scheduled.focus();await user.keyboard('{ArrowLeft}');
  expect(screen.getByRole('tab',{name:'One-time 1'})).toHaveAttribute('aria-selected','true');
  expect(screen.getByRole('tab',{name:'One-time 1'})).toHaveFocus();
});

it('leaves a recurring task created from the one-time editor visible in Scheduled',async()=>{
  const be=syntheticBackend();setTransport(be.transport);const user=userEvent.setup();render(<TasksScreen />);
  await user.click(screen.getByRole('button',{name:'+ Add task'}));
  await user.type(screen.getByLabelText('What needs to be done?'),'Study weekly');
  await user.click(screen.getByText('More options'));
  await user.selectOptions(screen.getByLabelText('Repeat'),'WEEKLY');
  await user.click(screen.getByRole('button',{name:'Add task'}));
  await waitFor(()=>expect(screen.getByRole('tab',{name:'Scheduled 1'})).toHaveAttribute('aria-selected','true'));
  expect(screen.getByRole('button',{name:'Edit Study weekly'})).toBeInTheDocument();
});

it('applies a prefilled single-meeting room change from daily review and keeps Undo after the notice disappears',async()=>{
  const be=autoBackend();be.settings.enabled=true;const c=notice();be.control.updates.push(c);be.tasks.push({...routine});
  const calls:{cmd:string;args:unknown}[]=[];
  setTransport(async(cmd,args)=>{
    if(cmd==='schedule_change_apply'){calls.push({cmd,args});be.control.updates=[];return {id:'adjustment',task_id:'routine',revision:5,replayed:false,active:true};}
    if(cmd==='schedule_change_undo'){calls.push({cmd,args});return {id:'adjustment',task_id:'routine',revision:6,replayed:false,active:false};}
    return be.transport(cmd,args);
  });
  const changed=vi.fn(async()=>{});const user=userEvent.setup();render(<DailyReview onTasksChanged={changed} />);
  await user.click(await screen.findByRole('button',{name:'Update schedule'}));
  const form=await screen.findByRole('form',{name:'Schedule change: Lecture moved'});
  await waitFor(()=>expect(within(form).getByLabelText('Routine')).toHaveValue('routine'));
  expect(within(form).getByLabelText('Applies to')).toHaveValue('this');
  expect(within(form).getByLabelText('Meeting date')).toHaveValue('2026-10-22');
  expect(within(form).getByLabelText('New room (optional)')).toHaveValue('B');
  expect(calls).toHaveLength(0);
  await user.click(within(form).getByRole('button',{name:'Apply schedule change'}));
  expect(await screen.findByRole('button',{name:'Undo schedule change'})).toBeInTheDocument();
  expect(calls[0]).toEqual({cmd:'schedule_change_apply',args:{req:{candidate_id:c.id,payload_hash:c.payload_hash,task_id:'routine',expected_revision:4,scope:'this',date:'2026-10-22',new_time:null,location:'B'}}});
  expect(be.tasks).toHaveLength(1);expect(be.control.batches).toHaveLength(0);
  await user.click(screen.getByRole('button',{name:'Undo schedule change'}));
  expect(await screen.findByText('Schedule change undone.')).toBeInTheDocument();
  expect(calls[1]).toEqual({cmd:'schedule_change_undo',args:{req:{id:'adjustment',expected_revision:5}}});
});

it('requires explicit target/scope when uncertain and sends the chosen weekday-only following scope',async()=>{
  const be=syntheticBackend();be.tasks.push({...routine},{...routine,id:'other'});const calls:unknown[]=[];
  setTransport(async(cmd,args)=>{if(cmd==='schedule_change_apply'){calls.push(args);return {id:'a',task_id:'routine',revision:5,replayed:false,active:true};}return be.transport(cmd,args);});
  const c={...notice(),excerpt:'Algorithmen wird am Donnerstag in Raum B verlegt.'};const user=userEvent.setup();render(<ScheduleNotice candidate={c} onApplied={async()=>{}} />);
  await user.click(screen.getByRole('button',{name:'Update schedule'}));
  await screen.findByLabelText('Routine');
  expect(screen.getByLabelText('Routine')).toHaveValue('');expect(screen.getByLabelText('Applies to')).toHaveValue('');
  expect(screen.getByRole('button',{name:'Apply schedule change'})).toBeDisabled();
  await user.selectOptions(screen.getByLabelText('Routine'),'routine');await user.selectOptions(screen.getByLabelText('Applies to'),'following');
  fireEvent.change(screen.getByLabelText('New time (optional)'),{target:{value:'12:00'}});
  expect(screen.getByText(/Only this weekday changes/)).toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:'Apply schedule change'}));
  await waitFor(()=>expect(calls).toHaveLength(1));
  expect(calls[0]).toMatchObject({req:{scope:'following',date:'2026-10-22',new_time:'12:00'}});
});

it('shows a source/target refusal without claiming success or retrying a paid/model call',async()=>{
  const be=syntheticBackend();be.tasks.push({...routine});let calls=0;
  setTransport(async(cmd,args)=>{if(cmd==='schedule_change_apply'){calls++;throw {code:'Conflict',message:'The routine changed.'};}return be.transport(cmd,args);});
  const applied=vi.fn();const user=userEvent.setup();render(<ScheduleNotice candidate={notice()} onApplied={applied} />);
  await user.click(screen.getByRole('button',{name:'Update schedule'}));
  await waitFor(()=>expect(screen.getByRole('button',{name:'Apply schedule change'})).toBeEnabled());
  await user.click(screen.getByRole('button',{name:'Apply schedule change'}));
  expect(await screen.findByRole('alert')).toHaveTextContent('The routine changed.');expect(applied).not.toHaveBeenCalled();expect(calls).toBe(1);
});

it('keeps temporary, permanent and unclear scope distinct and does not guess between old/new rooms',()=>{
  expect(suggestedScope('nur am Donnerstag')).toBe('this');expect(suggestedScope('ab sofort jeden Donnerstag')).toBe('following');
  expect(suggestedScope('ab sofort vorübergehend bis zum 30.10.')).toBe('');expect(suggestedScope('Donnerstag')).toBe('');
  expect(suggestedRoom('statt Raum A jetzt Raum B')).toBe('');expect(suggestedRoom('in Raum 0.01.')).toBe('0.01');
});

it('shows a schedule notice instead of a create-task form and identifies an applied decision in source history',()=>{
  const {rerender}=render(<CandidateCard c={notice()} onDone={async()=>{}} />);
  expect(screen.queryByRole('button',{name:'Create task'})).toBeNull();
  expect(screen.getByRole('link',{name:'Review it on Today'})).toHaveAttribute('href','#/today');
  rerender(<CandidateCard c={{...notice(),status:'rejected',schedule_active:true}} onDone={async()=>{}} />);
  expect(screen.getByText('Schedule updated')).toBeInTheDocument();
  expect(screen.queryByText(/status rejected/)).toBeNull();
});
