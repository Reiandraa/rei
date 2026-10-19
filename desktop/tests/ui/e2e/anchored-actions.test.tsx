// Synthetic IPC and DOM rectangles. This is not native screenshot/geometry evidence.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { TasksScreen } from '../../../src/screens/Planner';
import { dateKey } from '../../../src/schedule';
import { App } from '../../../src/App';
import { DeleteAction } from '../../../src/components/DeleteAction';
import { OccurrenceControls } from '../../../src/components/OccurrenceControls';
import { setTransport, type AgendaItem } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';

const item:AgendaItem={kind:'occurrence',id:'routine',title:'Lecture',local_start:'2026-10-19T10:00:00',all_day:false,state:'Normal',original_local:'2026-10-19T10:00:00',task_revision:7,recurring:true,read_only:false,conflict:false};
afterEach(()=>{cleanup();setTransport(null);vi.restoreAllMocks();});

it('keeps Delete in place, opens confirmation outside the task row and cancels without writing',async()=>{
  const be=syntheticBackend();be.tasks.push({id:'once',revision:5,title:'Submit report',status:'todo',recurrence:null,due_date:null});setTransport(be.transport);window.location.hash='#/tasks';const user=userEvent.setup();render(<App/>);
  const trigger=await screen.findByRole('button',{name:'Delete: Submit report'});const row=trigger.closest('li')!;const before=row.textContent;
  await user.click(trigger);const popup=screen.getByRole('dialog',{name:'Delete task: Submit report'});
  expect(popup.parentElement).toBe(document.body);expect(row).not.toContainElement(popup);expect(row.textContent).toBe(before);expect(screen.getByRole('button',{name:'Delete: Submit report'})).toBe(trigger);
  expect(within(popup).getByRole('button',{name:'Keep task'})).toHaveFocus();expect(be.calls.some(c=>c.cmd==='task_delete')).toBe(false);
  await user.keyboard('{Escape}');expect(screen.queryByRole('dialog')).toBeNull();expect(trigger).toHaveFocus();
  await user.click(trigger);await user.click(screen.getByRole('heading',{name:'Tasks',level:1}));expect(screen.queryByRole('dialog')).toBeNull();expect(be.tasks).toHaveLength(1);
});

it('sends one exact revision-bound deletion only after explicit confirmation and prevents double submit',async()=>{
  const be=syntheticBackend();be.tasks.push({id:'once',revision:5,title:'Submit report',status:'todo',recurrence:null,due_date:null});
  let release!:()=>void;let calls=0;const gate=new Promise<void>(resolve=>{release=resolve;});
  setTransport(async(cmd,args)=>{if(cmd==='task_delete'){calls++;await gate;}return be.transport(cmd,args);});
  window.location.hash='#/tasks';const user=userEvent.setup();render(<App/>);
  await user.click(await screen.findByRole('button',{name:'Delete: Submit report'}));const confirm=screen.getByRole('button',{name:'Confirm delete: Submit report'});await user.click(confirm);expect(confirm).toBeDisabled();fireEvent.click(confirm);expect(calls).toBe(1);expect(be.tasks).toHaveLength(1);
  await act(async()=>release());await waitFor(()=>expect(be.tasks).toHaveLength(0));expect(be.calls.find(c=>c.cmd==='task_delete')!.args).toEqual({req:{id:'once',expected_revision:5,confirm:true}});expect(screen.queryByRole('dialog')).toBeNull();
});

it('keeps the schedule trigger and content in place while a floating menu dismisses or skips only this occurrence',async()=>{
  const edit=vi.fn(async()=>{});const user=userEvent.setup();const view=render(<section aria-label="Timeline row"><OccurrenceControls item={item} onEdit={edit}/></section>);
  const row=screen.getByRole('region',{name:'Timeline row'});const trigger=screen.getByRole('button',{name:'More options: Lecture'});const before=row.textContent;
  await user.click(trigger);const popup=screen.getByRole('dialog',{name:'Schedule options: Lecture'});expect(popup.parentElement).toBe(document.body);expect(row).not.toContainElement(popup);expect(row.textContent).toBe(before);expect(screen.getByRole('button',{name:'More options: Lecture'})).toBe(trigger);
  await user.keyboard('{Escape}');expect(screen.queryByRole('dialog')).toBeNull();expect(trigger).toHaveFocus();expect(edit).not.toHaveBeenCalled();
  await user.click(trigger);await user.click(screen.getByRole('button',{name:'Skip this time: Lecture'}));expect(edit).toHaveBeenCalledOnce();expect(edit).toHaveBeenCalledWith(item,'skip');expect(screen.queryByRole('dialog')).toBeNull();expect(trigger).toHaveAttribute('aria-expanded','false');
  await user.click(trigger);view.unmount();expect(screen.queryByRole('dialog')).toBeNull();
});

it('flips a confirmation above a low trigger, stays inside a narrow viewport and follows scroll',async()=>{
  const rect=(x:number,y:number,w:number,h:number)=>({x,y,width:w,height:h,top:y,left:x,right:x+w,bottom:y+h,toJSON:()=>({})});
  let y=window.innerHeight-36;
  vi.spyOn(HTMLElement.prototype,'getBoundingClientRect').mockImplementation(function(this:HTMLElement){return this.classList.contains('anchored-popover')?rect(0,0,260,150):rect(5,y,40,24);});
  const onConfirm=vi.fn(async()=>true);const user=userEvent.setup();render(<DeleteAction title="Small screen item" onConfirm={onConfirm}/>);await user.click(screen.getByRole('button',{name:'Delete: Small screen item'}));const popup=screen.getByRole('dialog');expect(popup.style.top).toBe(`${y-158}px`);expect(popup.style.left).toBe('8px');
  y=100;fireEvent.scroll(window);expect(popup.style.top).toBe('132px');expect(onConfirm).not.toHaveBeenCalled();
});

it('keeps a refused deletion reviewable without claiming success',async()=>{
  const user=userEvent.setup();render(<DeleteAction title="Changed task" onConfirm={async()=>{throw {code:'Conflict',message:'The task changed; reload before deleting.'};}}/>);
  await user.click(screen.getByRole('button',{name:'Delete: Changed task'}));await user.click(screen.getByRole('button',{name:'Confirm delete: Changed task'}));expect(await screen.findByRole('alert')).toHaveTextContent('The task changed; reload before deleting.');expect(screen.getByRole('dialog')).toBeInTheDocument();await user.click(screen.getByRole('button',{name:'Keep task'}));expect(screen.queryByRole('dialog')).toBeNull();
});

const routine=()=>({id:'routine',revision:4,title:'Weekly lecture',notes:'Room A',status:'todo',due_date:null,due_at_local:null,recurrence:{series_id:'s',rrule:'FREQ=WEEKLY;BYDAY=MO,TH',dtstart_local:'2026-10-19T09:00:00',tz:'Europe/Berlin',weekday_times:{MO:'09:00',TH:'10:00'}}});

it('offers an explicit scheduled editor, prefills weekdays/times and saves the new pattern on the same task',async()=>{
  const be=syntheticBackend();be.tasks.push(routine());setTransport(be.transport);const user=userEvent.setup();render(<TasksScreen/>);
  await user.click(await screen.findByRole('tab',{name:'Scheduled 1'}));await user.click(screen.getByRole('button',{name:'Edit schedule: Weekly lecture'}));
  const dialog=screen.getByRole('dialog',{name:'Edit schedule'});expect(within(dialog).getByRole('button',{name:'Monday'})).toHaveAttribute('aria-pressed','true');expect(within(dialog).getByLabelText('Thursday time')).toHaveValue('10:00');expect(within(dialog).queryByLabelText('Due date (optional)')).toBeNull();
  await user.click(within(dialog).getByRole('button',{name:'Monday'}));await user.click(within(dialog).getByRole('button',{name:'Tuesday'}));fireEvent.change(within(dialog).getByLabelText('Tuesday time'),{target:{value:'14:00'}});fireEvent.change(within(dialog).getByLabelText('Apply changes from'),{target:{value:'2099-01-01'}});
  await user.click(within(dialog).getByRole('button',{name:'Save schedule'}));expect(await screen.findByText('Changes saved.')).toBeInTheDocument();expect(be.tasks).toHaveLength(1);expect(screen.getByText(/Tue 14:00/)).toHaveTextContent('Thu 10:00');
  expect(be.calls.find(c=>c.cmd==='task_update')!.args).toEqual({req:{id:'routine',expected_revision:4,title:'Weekly lecture',notes:'Room A',schedule:{effective_from:'2099-01-01',recurrence:{rrule:'FREQ=WEEKLY;BYDAY=TU,TH',tz:'Europe/Berlin',dtstart_local:'2026-10-19T09:00:00',weekday_times:{TU:'14:00',TH:'10:00'}}}}});expect(be.calls.some(c=>c.cmd==='task_create')).toBe(false);
});

it('does not rewrite a routine schedule when only its title or notes change',async()=>{
  const be=syntheticBackend();be.tasks.push(routine());setTransport(be.transport);const user=userEvent.setup();render(<TasksScreen/>);
  await user.click(await screen.findByRole('tab',{name:'Scheduled 1'}));await user.click(screen.getByRole('button',{name:'Edit schedule: Weekly lecture'}));await user.clear(screen.getByLabelText('Notes'));await user.type(screen.getByLabelText('Notes'),'Bring notebook');await user.click(screen.getByRole('button',{name:'Save schedule'}));await screen.findByText('Changes saved.');
  expect(be.calls.find(c=>c.cmd==='task_update')!.args).toEqual({req:{id:'routine',expected_revision:4,title:'Weekly lecture',notes:'Bring notebook'}});
});

it('retains schedule edits after a revision refusal without retrying or claiming a save',async()=>{
  const be=syntheticBackend();be.tasks.push(routine());let writes=0;setTransport(async(cmd,args)=>{if(cmd==='task_update'){writes++;throw {code:'Conflict',message:'This routine changed. Reopen its latest schedule.'};}return be.transport(cmd,args);});const user=userEvent.setup();render(<TasksScreen/>);
  await user.click(await screen.findByRole('tab',{name:'Scheduled 1'}));await user.click(screen.getByRole('button',{name:'Edit schedule: Weekly lecture'}));fireEvent.change(screen.getByLabelText('Thursday time'),{target:{value:'11:30'}});await user.click(screen.getByRole('button',{name:'Save schedule'}));
  expect(await screen.findByRole('alert')).toHaveTextContent('This routine changed');expect(screen.getByLabelText('Thursday time')).toHaveValue('11:30');expect(screen.queryByText('Changes saved.')).toBeNull();expect(writes).toBe(1);expect(be.tasks[0].recurrence.weekday_times.TH).toBe('10:00');
});

it('opens the same prefilled schedule editor from the timeline title and the menu action above Skip',async()=>{
  const be=syntheticBackend();const task=routine();be.tasks.push(task);const day=dateKey(new Date());be.agenda.push({...item,title:task.title,local_start:`${day}T10:00:00`,original_local:`${day}T10:00:00`,task_revision:task.revision});setTransport(be.transport);const user=userEvent.setup();render(<App/>);
  const timeline=screen.getByRole('complementary',{name:'Daily timeline'});await user.click(await within(timeline).findByRole('button',{name:'Weekly lecture'}));
  expect(screen.getByRole('dialog',{name:'Edit schedule'})).toBeInTheDocument();expect(screen.getByLabelText('Monday time')).toHaveValue('09:00');await user.click(screen.getByRole('button',{name:'Cancel'}));
  const trigger=within(timeline).getByRole('button',{name:'More options: Weekly lecture'});await user.click(trigger);
  const menu=screen.getByRole('dialog',{name:'Schedule options: Weekly lecture'});expect(within(menu).getAllByRole('button').map(b=>b.getAttribute('aria-label'))).toEqual(['Edit schedule: Weekly lecture','Skip this time: Weekly lecture']);
  await user.click(within(menu).getByRole('button',{name:'Edit schedule: Weekly lecture'}));expect(screen.queryByRole('dialog',{name:'Schedule options: Weekly lecture'})).toBeNull();expect(screen.getByRole('dialog',{name:'Edit schedule'})).toBeInTheDocument();expect(screen.getByLabelText('Thursday time')).toHaveValue('10:00');
  expect(be.calls.some(c=>['task_update','task_create','occurrence_edit'].includes(c.cmd))).toBe(false);await user.click(screen.getByRole('button',{name:'Cancel'}));expect(trigger).toHaveFocus();
});
