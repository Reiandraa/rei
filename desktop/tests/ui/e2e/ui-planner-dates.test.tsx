// jsdom tests for the ui-planner-dates slice against a SYNTHETIC transport (recorded calls).
// The real plan/undo/date rules are enforced and tested in src-tauri/tests/ui_planner_dates.rs.
// jsdom is not native layout, live provider or model-quality evidence.
import { act, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { setTransport, type CommandName, type Task } from '../../../src/ipc';
import { DayTimeline } from '../../../src/components/DayTimeline';
import { App } from '../../../src/App';
import { isoDate } from '../../../src/screens/Planner';
import { syntheticBackend } from '../fake-backend';
import { TasksScreen } from '../../../src/screens/Planner';
import { LocalAiSettings } from '../../../src/screens/SettingsScreen';
import { DailyReview } from '../../../src/screens/DailyReview';
import { autoBackend, cand } from '../auto-backend';

afterEach(() => setTransport(null));

function task(extra: Partial<Task> = {}): Task {
  return {
    id: 'fixture-task', revision: 3, title: 'Upload report', notes: '', status: 'todo', priority: 0, energy: null,
    duration_min: null, due_date: null, due_at_utc: '2026-10-23T10:00:00Z', due_tz: 'Europe/Berlin',
    due_at_local: '2026-10-23T12:00:00', recurrence: null, reminder_offsets_min: [], created_at: '', updated_at: '',
    plan_date: null, plan_time: null, plan_duration_min: null, plan_tz: null, undo_action: null, ...extra,
  };
}

/** Synthetic task store that applies the commands the screen sends, so reloads show the result. */
function taskBackend(initial: Task) {
  let t = { ...initial };
  const calls: { cmd: CommandName; req?: Record<string, unknown> }[] = [];
  const transport = async (cmd: CommandName, args?: Record<string, unknown>) => {
    const req = (args as { req?: Record<string, unknown> } | undefined)?.req;
    calls.push({ cmd, req });
    switch (cmd) {
      case 'task_list': return [t];
      case 'task_plan':
        t = { ...t, revision: t.revision + 1, plan_date: req!.plan_date as string, plan_time: (req!.plan_time as string) ?? null,
          plan_duration_min: (req!.duration_min as number) ?? null, undo_action: 'plan' };
        return t;
      case 'task_unplan':
        t = { ...t, revision: t.revision + 1, plan_date: null, plan_time: null, plan_duration_min: null, undo_action: 'unplan' };
        return t;
      case 'task_undo':
        t = { ...t, revision: t.revision + 1, plan_date: '2026-10-21', plan_time: '14:00', undo_action: null };
        return t;
      case 'task_complete': t = { ...t, revision: t.revision + 1, status: 'done', undo_action: 'complete' }; return t;
      case 'task_reopen': t = { ...t, revision: t.revision + 1, status: 'todo', undo_action: null }; return t;
      default: throw { code: 'Unavailable', message: `not in this fixture: ${cmd}` };
    }
  };
  return { transport, calls };
}

describe('work plans in Tasks (synthetic transport)', () => {
  it('plans separately from the deadline, unplans and undoes with exact revisions', async () => {
    const b = taskBackend(task());
    setTransport(b.transport);
    const user = userEvent.setup();
    render(<TasksScreen />);
    expect(await screen.findByText(/Not planned/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Plan Upload report' }));
    const form = screen.getByRole('form', { name: 'Plan Upload report' });
    const date = within(form).getByLabelText('Work on') as HTMLInputElement;
    await user.clear(date);
    await user.type(date, '2026-10-21');
    await user.type(within(form).getByLabelText('Start (optional)'), '14:00');
    await user.click(within(form).getByRole('button', { name: 'Save plan' }));
    expect(await screen.findByText(/Planned 2026-10-21 14:00 \(60 min\)/)).toBeInTheDocument();
    const plan = b.calls.find((c) => c.cmd === 'task_plan')!.req!;
    expect(plan).toMatchObject({ id: 'fixture-task', expected_revision: 3, plan_date: '2026-10-21', plan_time: '14:00', duration_min: 60 });
    expect(typeof plan.tz).toBe('string');
    // The deadline is still shown and never relabelled as work time.
    expect(screen.getAllByText(/Due /).length).toBeGreaterThan(0);
    await user.click(screen.getByRole('button', { name: 'Unplan Upload report' }));
    expect(await screen.findByText(/Not planned/)).toBeInTheDocument();
    expect(b.calls.find((c) => c.cmd === 'task_unplan')!.req).toEqual({ id: 'fixture-task', expected_revision: 4 });
    await user.click(screen.getByRole('button', { name: 'Undo unplan' }));
    await waitFor(() => expect(b.calls.some((c) => c.cmd === 'task_undo')).toBe(true));
    expect(b.calls.find((c) => c.cmd === 'task_undo')!.req).toEqual({ id: 'fixture-task', expected_revision: 5 });
    expect(await screen.findByText(/Planned 2026-10-21 14:00/)).toBeInTheDocument();
  });

  it('reopens a completed task with its exact revision and offers Undo after completing', async () => {
    const b = taskBackend(task({ status: 'done', revision: 7 }));
    setTransport(b.transport);
    const user = userEvent.setup();
    render(<TasksScreen />);
    await user.click(await screen.findByRole('button', { name: 'Reopen Upload report' }));
    expect(b.calls.find((c) => c.cmd === 'task_reopen')!.req).toEqual({ id: 'fixture-task', expected_revision: 7 });
    await user.click(await screen.findByRole('button', { name: 'Complete: Upload report' }));
    await waitFor(() => expect(b.calls.some((c) => c.cmd === 'task_complete')).toBe(true));
    expect(await screen.findByRole('button', { name: 'Undo complete' })).toBeInTheDocument();
  });
});

describe('automatic date/time display in grouped review (synthetic transport)', () => {
  it('shows interpreted relative deadlines with their source anchor and keeps unclear ones visible', async () => {
    const b = autoBackend();
    b.settings.enabled = true;
    b.control.actionable.push(
        cand(1, { due_date: '2026-10-23', due_time: '12:00', date_text: 'bis Freitag um 12 Uhr', date_basis: 'relative',
          date_anchor: '2026-10-21T08:00:00Z', date_zone: 'Europe/Berlin', date_note: "Interpreted from the message's send date in Europe/Berlin" }),
        cand(2, { due_date: null, date_status: 'ambiguous', date_text: 'nächsten Freitag', date_basis: null, date_note: '"next <weekday>" can mean this or next week' }),
    );
    setTransport(b.transport);
    render(<DailyReview />);
    expect(await screen.findByText('2026-10-23, 12:00')).toBeInTheDocument();
    expect(screen.getByText(/from “bis Freitag um 12 Uhr”, sent 2026-10-21 \(Europe\/Berlin\)/)).toBeInTheDocument();
    expect(screen.getByText(/deadline time 12:00 Europe\/Berlin/)).toBeInTheDocument();
    expect(screen.getByText(/can mean this or next week/)).toBeInTheDocument();
  });
});

describe('owner date/time edits in grouped review (synthetic transport)', () => {
  it('shows the send day in the resolver zone and keeps the stated time when only the date is changed', async () => {
    const b = autoBackend();
    b.settings.enabled = true;
    // Sent Fri 23 Oct 00:30 Berlin = Thu 22:30 UTC: the UTC day would be the wrong provenance.
    b.control.actionable.push(cand(1, { due_date: '2026-10-23', due_time: '12:00', date_text: 'heute um 12 Uhr', date_basis: 'relative',
      date_anchor: '2026-10-22T22:30:00Z', date_zone: 'Europe/Berlin', date_note: '' }));
    setTransport(b.transport);
    render(<DailyReview />);
    expect(await screen.findByText(/sent 2026-10-23 \(Europe\/Berlin\)/)).toBeInTheDocument();
    const title = b.control.actionable[0].title;
    const date = screen.getByLabelText(`Due date for ${title}`);
    await userEvent.clear(date);
    await userEvent.type(date, '2026-10-24');
    expect(screen.getByLabelText(`Due time for ${title}`)).toHaveValue('12:00');
    await userEvent.click(screen.getByRole('button', { name: /Approve 1 selected/ }));
    await waitFor(() => expect(b.control.batches).toHaveLength(1));
    expect(b.control.batches[0][0]).toMatchObject({ date_resolved: true, due_date: '2026-10-24', due_time: '12:00' });
  });
});

describe('Local AI settings (synthetic transport)', () => {
  it('lists installed models, and the selection is saved for the worker without changing opt-in', async () => {
    const calls: { cmd: CommandName; req?: Record<string, unknown> }[] = [];
    let model = 'qwen3:8b';
    setTransport(async (cmd, args) => {
      const req = (args as { req?: Record<string, unknown> } | undefined)?.req;
      calls.push({ cmd, req });
      if (cmd === 'model_list') return { state: 'ready', models: ['llama3.2:latest', 'qwen3:8b'] };
      if (cmd === 'auto_settings_get' || cmd === 'auto_settings_set') {
        if (req?.model) model = req.model as string;
        return { enabled: true, paused: false, model, worker_state: 'idle', worker_detail: '', backoff_until: null, date_zone: (req?.date_zone as string) ?? 'Europe/Berlin' };
      }
      throw { code: 'Unavailable', message: cmd };
    });
    const user = userEvent.setup();
    render(<LocalAiSettings />);
    const select = await screen.findByLabelText('Installed model');
    await user.selectOptions(select, 'llama3.2:latest');
    await waitFor(() => expect(calls.some((c) => c.cmd === 'auto_settings_set')).toBe(true));
    expect(calls.find((c) => c.cmd === 'auto_settings_set')!.req).toEqual({ enabled: true, paused: false, model: 'llama3.2:latest' });
    expect(await screen.findByText(/now uses llama3.2:latest/)).toBeInTheDocument();
  });

  it('says honestly when Ollama is not running; no invented models', async () => {
    setTransport(async (cmd) => {
      if (cmd === 'model_list') return { state: 'not_running', models: [] };
      if (cmd === 'auto_settings_get') return { enabled: false, paused: false, model: 'qwen3:8b', worker_state: 'offline', worker_detail: '', backoff_until: null, date_zone: 'Europe/Berlin' };
      throw { code: 'Unavailable', message: cmd };
    });
    render(<LocalAiSettings />);
    expect(await screen.findByText(/Ollama is not running/)).toBeInTheDocument();
    expect(screen.queryByLabelText('Installed model')).toBeNull();
  });
});

// The approved visual arrangement is represented by semantic panes here. This is not screenshot evidence.
describe('integrated approved planner flow (synthetic IPC)', () => {
  it('edits title/notes without losing the timed deadline, and keeps a refused edit open', async () => {
    const b = syntheticBackend();
    b.tasks.push(task());
    setTransport(b.transport); window.location.hash = '#/tasks';
    const user = userEvent.setup(); render(<App />);
    await user.click(await screen.findByRole('button', { name: 'Edit Upload report' }));
    const dialog = screen.getByRole('dialog', { name: 'Edit task' });
    expect(within(dialog).getByLabelText('Due time (optional)')).toHaveValue('12:00');
    await user.type(within(dialog).getByLabelText('Notes'), 'Bring the appendix');
    await user.click(within(dialog).getByRole('button', { name: 'Save task' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(b.calls.find(c => c.cmd === 'task_update')?.args).toEqual({ req: { id: 'fixture-task', expected_revision: 3, title: 'Upload report', notes: 'Bring the appendix' } });
    expect(b.tasks[0].due_at_utc).toBe('2026-10-23T10:00:00Z');
    await user.click(screen.getByRole('button', { name: 'Edit Upload report' }));
    b.tasks[0].revision++; // Another writer changed the actual store after the editor opened.
    await user.click(screen.getByRole('button', { name: 'Save task' }));
    expect(await within(screen.getByRole('dialog')).findByRole('alert')).toHaveTextContent('revision mismatch');
    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.getByRole('button', { name: 'Edit Upload report' })).toHaveFocus();
  });

  it('removes only the work plan from Today, keeps the task/deadline, then restores it with Undo', async () => {
    const b = syntheticBackend(), day = isoDate(new Date());
    const deadline = new Date(); deadline.setDate(deadline.getDate()+3);
    b.tasks.push(task({ due_date: isoDate(deadline), due_at_utc: null, due_at_local: null, due_tz: null, plan_date: day, plan_time: '14:00', plan_duration_min: 60, plan_tz: Intl.DateTimeFormat().resolvedOptions().timeZone }));
    setTransport(b.transport); window.location.hash = '#/today';
    const user = userEvent.setup(); render(<App />);
    const planned = await screen.findByRole('region', { name: 'Planned tasks' });
    await within(planned).findByText('Upload report');
    const timeline = screen.getByRole('complementary', { name: 'Daily timeline' });
    expect(within(timeline).getByText('14:00–15:00')).toBeInTheDocument();
    await user.click(within(planned).getByRole('button', { name: 'Unplan Upload report' }));
    const unscheduled = screen.getByRole('region', { name: 'Unscheduled tasks' });
    expect(await within(unscheduled).findByText('Upload report')).toBeInTheDocument();
    expect(b.tasks[0].due_date).toBe(isoDate(deadline));
    expect(within(planned).queryByText('Upload report')).toBeNull();
    expect(within(timeline).queryByText('Upload report')).toBeNull();
    await user.click(within(unscheduled).getByRole('button', { name: 'Undo unplan' }));
    expect(await within(planned).findByText('Upload report')).toBeInTheDocument();
    expect(within(timeline).getByText('14:00–15:00')).toBeInTheDocument();
    expect(b.calls.find(c => c.cmd === 'task_undo')?.args).toEqual({ req: { id: 'fixture-task', expected_revision: 4 } });
  });

  it('opens an empty editor with focus, cancels without writing, and shows no account/model forms on Today', async () => {
    const b = autoBackend(); setTransport(b.transport); window.location.hash = '#/today';
    const user = userEvent.setup(); render(<App />);
    await screen.findByText('Nothing planned yet. Choose a task below and give it some time.');
    expect(screen.queryByLabelText('Local model')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Turn on automatic assistance' })).toBeNull();
    expect(await screen.findByRole('link', { name: /Set up automatic assistance/ })).toHaveAttribute('href', '#/settings');
    const trigger = screen.getByRole('button', { name: '+ Add task' });
    await user.click(trigger);
    expect(screen.getByLabelText('What needs to be done?')).toHaveFocus();
    expect(document.querySelector('.shell')).toHaveAttribute('inert');
    await user.keyboard('{Escape}');
    expect(trigger).toHaveFocus();
    expect(document.querySelector('.shell')).not.toHaveAttribute('inert');
    expect(b.calls.some(c => c.cmd === 'task_create')).toBe(false);
  });
});


describe('independent Settings sections preserve current worker choices', () => {
  it('changing the installed model keeps an opt-in changed after the model section loaded', async () => {
    let enabled = false, model = 'old:1';
    const writes: Record<string, unknown>[] = [];
    setTransport(async (cmd,args) => {
      if (cmd === 'model_list') return { state: 'ready', models: ['old:1','new:2'] };
      if (cmd === 'auto_settings_set') { writes.push(args!.req as Record<string,unknown>); return args!.req; }
      if (cmd === 'auto_settings_get') return { enabled, paused: false, model, date_zone: 'Europe/Berlin' };
      throw { code:'Unavailable', message:cmd };
    });
    const user = userEvent.setup(); render(<LocalAiSettings />);
    await screen.findByLabelText('Installed model');
    enabled = true; model = 'old:1'; // Changed in Automatic assistance while this section stays mounted.
    await user.selectOptions(screen.getByLabelText('Installed model'), 'new:2');
    await waitFor(() => expect(writes).toHaveLength(1));
    expect(writes[0]).toEqual({ enabled:true, paused:false, model:'new:2' });
  });
});


describe('timeline boundaries and navigation', () => {
  it('keeps calendar events read-only and clips an overnight event to the selected day', () => {
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    render(<DayTimeline day="2026-10-22" tasks={[]} editTask={vi.fn()} editOccurrence={vi.fn()} events={[{ id:'overnight', revision:1, title:'Overnight seminar', all_day:false, start_local:'2026-10-21T23:00:00', end_local:'2026-10-22T01:00:00', start_utc:null, end_utc:null, tz:zone, provider:'synthetic', read_only:true }]} items={[{ kind:'event', id:'overnight', title:'Overnight seminar', local_start:'2026-10-21T23:00:00', all_day:false, state:'Normal', original_local:null, task_revision:null, recurring:false, read_only:true, conflict:false }]} />);
    expect(screen.getByText('00:00–01:00')).toBeInTheDocument();
    expect(screen.getByText('Read-only')).toBeInTheDocument();
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('labels a different saved zone separately instead of placing it at a false local hour', () => {
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone === 'Pacific/Honolulu' ? 'Europe/Berlin' : 'Pacific/Honolulu';
    render(<DayTimeline day="2026-10-23" events={[]} tasks={[task({due_tz:zone})]} editTask={vi.fn()} editOccurrence={vi.fn()} items={[{ kind:'task', id:'fixture-task', title:'Upload report', local_start:'2026-10-23T12:00:00', all_day:false, state:'todo', original_local:null, task_revision:3, recurring:false, read_only:false, conflict:false }]} />);
    const other = screen.getByRole('region', {name:'Other saved time zones'});
    expect(within(other).getByText(`Deadline · ${zone}`)).toBeInTheDocument();
    expect(within(other).getByText('2026-10-23 12:00:00')).toBeInTheDocument();
    expect(within(screen.getByLabelText('Scrollable hours')).queryByText('Upload report')).toBeNull();
  });

  it('does not let an older day response overwrite the day selected afterwards', async () => {
    const b = autoBackend(), firstDay = new Date();
    const later = new Date(firstDay); later.setDate(later.getDate()+7); const day = isoDate(later);
    let release: (v: unknown) => void = () => undefined;
    const old = new Promise(resolve => {release=resolve;}); let calls=0;
    b.tasks.push(task({plan_date:day, plan_time:'14:00', plan_duration_min:60, plan_tz:Intl.DateTimeFormat().resolvedOptions().timeZone}));
    setTransport(async (cmd,args) => {
      if(cmd === 'agenda_list') { calls++; if(calls===1) return old; return [{kind:'plan',id:'fixture-task',title:'Upload report',local_start:day+'T14:00:00',all_day:false,state:'todo',original_local:null,task_revision:3,recurring:false,read_only:false,conflict:false}]; }
      return b.transport(cmd,args);
    });
    window.location.hash='#/today'; const user=userEvent.setup(); render(<App />);
    await user.click(screen.getByRole('button',{name:'Next week'}));
    const timeline=screen.getByRole('complementary',{name:'Daily timeline'});
    expect(await within(timeline).findByText('Upload report')).toBeInTheDocument();
    await act(async()=>{release([]);await old;});
    expect(within(timeline).getByText('Upload report')).toBeInTheDocument();
    expect(within(timeline).getByText('14:00–15:00')).toBeInTheDocument();
  });
});
