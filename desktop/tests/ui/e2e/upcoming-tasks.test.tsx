// Synthetic task/agenda integration, not native UI or provider evidence.
import { cleanup, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { TodayScreen, isoDate } from '../../../src/screens/Planner';
import { LONG_DATE } from '../../../src/strings';
import { setTransport, type Task } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';

afterEach(()=>{cleanup();setTransport(null);});
const day=(offset:number)=>{const now=new Date();return isoDate(new Date(now.getFullYear(),now.getMonth(),now.getDate()+offset));};
const task=(id:string,extra:Partial<Task>={}):Task=>({id,revision:1,title:id,notes:'',status:'todo',priority:0,energy:null,duration_min:null,due_date:null,due_at_utc:null,due_tz:null,due_at_local:null,recurrence:null,reminder_offsets_min:[],created_at:'',updated_at:'',plan_date:null,plan_time:null,plan_duration_min:null,plan_tz:null,undo_action:null,...extra});
const upcoming=()=>screen.getByText('Upcoming tasks · next 6 days').closest('details')!;

it('keeps left-side lists for one-time tasks and deduplicates future plans/deadlines without hiding today’s meetings',async()=>{
  const be=syntheticBackend();const user=userEvent.setup();
  be.tasks.push(
    task('Both plan and deadline',{plan_date:day(1),plan_time:'13:30',due_date:day(3)}),
    task('Plan only',{plan_date:day(2),plan_time:'09:00'}),
    task('Timed deadline',{due_at_local:`${day(4)}T16:00:00`,due_tz:'Europe/Berlin'}),
    task('Already complete',{status:'done',due_date:day(1)}),
    task('Cancelled',{status:'cancelled',plan_date:day(1)}),
    task('Weekly lecture',{plan_date:day(0),due_date:day(1),recurrence:{series_id:'s',rrule:'FREQ=WEEKLY',dtstart_local:`${day(0)}T10:00:00`,tz:'Europe/Berlin'}}),
    task('Work today',{plan_date:day(0)}),task('Without a plan')
  );
  be.agenda.push(
    {kind:'occurrence',id:'Weekly lecture',title:'Weekly lecture',local_start:`${day(0)}T10:00:00`,all_day:false,state:'Normal',original_local:`${day(0)}T10:00:00`,task_revision:1,recurring:true,read_only:false,conflict:false},
    {kind:'event',id:'trip',title:'Trip to Frankfurt',local_start:day(0),all_day:true,state:'Normal',original_local:null,task_revision:null,recurring:false,read_only:false,conflict:false},
    {kind:'event',id:'trip-next',title:'Future trip',local_start:day(2),all_day:true,state:'Normal',original_local:null,task_revision:null,recurring:false,read_only:false,conflict:false}
  );
  setTransport(be.transport);render(<TodayScreen/>);
  const planned=screen.getByRole('region',{name:'Planned tasks'});
  await within(planned).findByText('Work today');expect(within(planned).queryByText('Weekly lecture')).toBeNull();
  const unplanned=screen.getByRole('region',{name:'Unscheduled tasks'});expect(within(unplanned).getByText('Without a plan')).toBeInTheDocument();expect(within(unplanned).queryByText('Weekly lecture')).toBeNull();
  const timeline=screen.getByRole('complementary',{name:'Daily timeline'});expect(within(timeline).getAllByText('Weekly lecture').length).toBeGreaterThan(0);expect(within(timeline).getByText('Trip to Frankfurt')).toBeInTheDocument();
  await user.click(screen.getByText('Upcoming tasks · next 6 days'));
  const list=within(upcoming());expect(list.getAllByRole('listitem')).toHaveLength(3);
  expect(list.getAllByRole('button').map(el=>el.textContent)).toEqual(['Both plan and deadline','Plan only','Timed deadline']);
  expect(list.getAllByText('Both plan and deadline')).toHaveLength(1);
  expect(list.getByText(`Planned ${LONG_DATE.format(new Date(`${day(1)}T12:00:00`))}, 13:30`)).toBeInTheDocument();
  expect(list.getByText(`Due ${LONG_DATE.format(new Date(`${day(3)}T12:00:00`))}`)).toBeInTheDocument();
  expect(list.getByText(/Due .*16:00/)).toBeInTheDocument();expect(list.queryByRole('checkbox')).toBeNull();
  await user.click(list.getByRole('button',{name:'Edit Both plan and deadline'}));expect(screen.getByRole('dialog',{name:'Edit task'})).toBeInTheDocument();
  expect(be.calls.some(c=>['task_update','task_create','occurrence_edit'].includes(c.cmd))).toBe(false);
});

it('uses the selected day’s next six days, includes day six and excludes today, later dates and undated tasks',async()=>{
  const be=syntheticBackend();be.tasks.push(task('Today deadline',{due_date:day(0)}),task('Day six',{due_date:day(6)}),task('Day seven',{due_date:day(7)}),task('Day eight',{due_date:day(8)}),task('Undated'));
  setTransport(be.transport);const user=userEvent.setup();render(<TodayScreen/>);
  await within(await screen.findByRole('region',{name:'Deadlines'})).findByText('Today deadline');
  await user.click(screen.getByText('Upcoming tasks · next 6 days'));
  expect(within(upcoming()).getAllByRole('listitem')).toHaveLength(1);expect(within(upcoming()).getByText('Day six')).toBeInTheDocument();
  await user.click(screen.getByRole('button',{name:'Next week'}));
  await within(screen.getByRole('region',{name:'Deadlines'})).findByText('Day seven');
  expect(within(upcoming()).getAllByRole('listitem')).toHaveLength(1);expect(within(upcoming()).getByText('Day eight')).toBeInTheDocument();
});
