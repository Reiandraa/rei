// Synthetic IPC integration; no native geometry, live account or model access.
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../../../src/App';
import { dateKey } from '../../../src/schedule';
import { setTransport } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';

afterEach(()=>{cleanup();setTransport(null);});
function setup(dueToday=false) {
  const be=syntheticBackend();const day=dateKey(new Date());
  be.tasks.push({id:'report',revision:5,title:'Submit report',notes:'Bring references',status:'todo',recurrence:null,due_date:dueToday?day:'2099-12-31',plan_date:day,plan_time:'10:00',plan_duration_min:60,plan_tz:Intl.DateTimeFormat().resolvedOptions().timeZone,undo_action:null});
  setTransport(be.transport);window.location.hash='#/today';render(<App/>);
  return be;
}
const timeline=()=>screen.getByRole('complementary',{name:'Daily timeline'});
const block=()=>within(timeline()).getByRole('article',{name:'Work plan: Submit report'});
const writes=(be:ReturnType<typeof syntheticBackend>)=>be.calls.filter(c=>['task_complete','task_reopen','task_unplan','task_delete','task_update','task_plan','occurrence_edit'].includes(c.cmd));

it('completes and reopens the same one-time task from the timeline, refreshing its deadline and work block together',async()=>{
  const be=setup(true);const user=userEvent.setup();
  await within(timeline()).findAllByRole('checkbox',{name:'Mark done: Submit report'});
  await user.click(within(block()).getByRole('checkbox'));
  await waitFor(()=>expect(within(timeline()).getAllByRole('checkbox',{name:'Reopen: Submit report'})).toHaveLength(2));
  expect(within(timeline()).getAllByRole('checkbox').every(el=>(el as HTMLInputElement).checked)).toBe(true);
  expect(be.tasks[0]).toMatchObject({status:'done',plan_time:'10:00',due_date:dateKey(new Date()),revision:6});
  await user.click(within(block()).getByRole('checkbox'));
  await waitFor(()=>expect(within(timeline()).getAllByRole('checkbox',{name:'Mark done: Submit report'})).toHaveLength(2));
  expect(writes(be)).toEqual([{cmd:'task_complete',args:{req:{id:'report',expected_revision:5}}},{cmd:'task_reopen',args:{req:{id:'report',expected_revision:6}}}]);
});

it('opens the existing task and plan editors from a stable floating menu without writing on open/cancel',async()=>{
  const be=setup();const user=userEvent.setup();const trigger=await within(timeline()).findByRole('button',{name:'More options: Submit report'});
  const before=block().textContent;
  await user.click(trigger);const menu=screen.getByRole('dialog',{name:'Task options: Submit report'});
  expect(menu.parentElement).toBe(document.body);expect(block()).not.toContainElement(menu);expect(block().textContent).toBe(before);
  expect(within(menu).getAllByRole('button').map(el=>el.textContent)).toEqual(['Edit task','Move plan','Remove from Today','Delete task']);
  await user.click(within(menu).getByRole('button',{name:'Edit task'}));
  expect(screen.getByRole('dialog',{name:'Edit task'})).toBeInTheDocument();expect(screen.getByLabelText('Notes')).toHaveValue('Bring references');
  await user.click(screen.getByRole('button',{name:'Cancel'}));expect(trigger).toHaveFocus();
  await user.click(trigger);await user.click(screen.getByRole('button',{name:'Move plan'}));
  expect(screen.getByRole('dialog',{name:'Plan Submit report'})).toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:'Cancel'}));expect(trigger).toHaveFocus();expect(writes(be)).toHaveLength(0);
});

it('removes only the work plan and leaves a task that can be restored through the existing Undo action',async()=>{
  const be=setup();const user=userEvent.setup();await user.click(await within(timeline()).findByRole('button',{name:'More options: Submit report'}));
  await user.click(screen.getByRole('button',{name:'Remove from Today'}));
  await waitFor(()=>expect(within(timeline()).queryByRole('article',{name:'Work plan: Submit report'})).toBeNull());
  expect(be.tasks[0]).toMatchObject({due_date:'2099-12-31',status:'todo',plan_date:null,revision:6});
  expect(writes(be)).toEqual([{cmd:'task_unplan',args:{req:{id:'report',expected_revision:5}}}]);
  await user.click(screen.getByRole('button',{name:'Undo unplan'}));
  await within(timeline()).findByRole('article',{name:'Work plan: Submit report'});expect(be.tasks[0]).toMatchObject({plan_time:'10:00',due_date:'2099-12-31',revision:7});
});

it('confirms deletion beside the unchanged ellipsis and offers the existing Undo delete after success',async()=>{
  const be=setup();const user=userEvent.setup();const trigger=await within(timeline()).findByRole('button',{name:'More options: Submit report'});
  await user.click(trigger);await user.click(screen.getByRole('button',{name:'Delete task'}));
  const confirmation=screen.getByRole('dialog',{name:'Delete task: Submit report'});
  expect(confirmation.parentElement).toBe(document.body);expect(within(confirmation).getByRole('button',{name:'Keep task'})).toHaveFocus();expect(writes(be)).toHaveLength(0);
  await user.click(within(confirmation).getByRole('button',{name:'Keep task'}));expect(trigger).toHaveFocus();expect(be.tasks).toHaveLength(1);
  await user.click(trigger);await user.click(screen.getByRole('button',{name:'Delete task'}));await user.click(screen.getByRole('button',{name:'Confirm delete: Submit report'}));
  await waitFor(()=>expect(be.tasks).toHaveLength(0));expect(writes(be)).toEqual([{cmd:'task_delete',args:{req:{id:'report',expected_revision:5,confirm:true}}}]);
  expect(await screen.findByRole('button',{name:'Undo delete'})).toBeInTheDocument();expect(screen.queryByRole('dialog')).toBeNull();
});

it('does not mark a task complete when the backend refuses and does not retry automatically',async()=>{
  const be=setup();let attempts=0;
  setTransport(async(cmd,args)=>{if(cmd==='task_complete'){attempts++;throw {code:'Conflict',message:'revision mismatch'};}return be.transport(cmd,args);});
  const user=userEvent.setup();await user.click(await within(timeline()).findByRole('checkbox',{name:'Mark done: Submit report'}));
  expect(await screen.findByRole('alert')).toBeInTheDocument();expect(within(block()).getByRole('checkbox')).not.toBeChecked();expect(be.tasks[0].status).toBe('todo');expect(attempts).toBe(1);
});
