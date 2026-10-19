// jsdom + SYNTHETIC IPC. These tests do not contact HIZ/Teams/Ollama or any real profile.
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { DailyReview } from '../../../src/screens/DailyReview';
import { App } from '../../../src/App';
import { setTransport } from '../../../src/ipc';
import { autoBackend, cand } from '../auto-backend';

afterEach(() => { setTransport(null); localStorage.clear(); vi.useRealTimers(); });

function open(be: ReturnType<typeof autoBackend>, configuration = false) {
  window.location.hash = '#/today';
  setTransport(be.transport);
  if (configuration) render(<DailyReview configuration />); else render(<App />);
  return userEvent.setup();
}

const lastReq = (be: ReturnType<typeof autoBackend>, cmd: string) =>
  [...be.calls].reverse().find((c) => c.cmd === cmd)?.args?.req as Record<string, unknown> | undefined;

describe('automatic study assistant (synthetic transport)', () => {
  it('explains the opt-in, enables it with the default local model and reports the polling change', async () => {
    const be = autoBackend();
    const user = open(be, true);
    expect(await screen.findByText(/including messages already imported and new or changed ones/)).toBeInTheDocument();
    expect(screen.getByText(/only while this app is open/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Turn on automatic assistance' }));
    expect(lastReq(be, 'auto_settings_set')).toEqual({ enabled: true, paused: false, model: 'qwen3.5:9b' });
    expect(lastReq(be, 'polling_set')).toEqual({ enabled: true, interval_minutes: 30 });
    expect(await screen.findByText(/now every 30 minutes \(was off\)\. Source selection is unchanged/)).toBeInTheDocument();
    expect(await screen.findByRole('heading', { name: 'Automatic assistance' })).toBeInTheDocument();
    expect(be.calls.some((c) => c.cmd === 'connection_select_sources')).toBe(false);
  });

  it('can enable without touching source polling', async () => {
    const be = autoBackend();
    const user = open(be, true);
    await user.click(await screen.findByRole('checkbox', { name: /every 30 minutes/ }));
    await user.click(screen.getByRole('button', { name: 'Turn on automatic assistance' }));
    await screen.findByRole('heading', { name: 'Automatic assistance' });
    expect(be.calls.some((c) => c.cmd === 'polling_set')).toBe(false);
  });

  it('shows progress, pauses, resumes, stops and retries without claiming all clear', async () => {
    const be = autoBackend();
    be.settings.enabled = true;
    be.control.queue = { queued: 2, running: 1, failed: 0, cancelled: 0 };
    const user = open(be, true);
    const status = await screen.findByRole('status', { name: 'Assistant status' });
    expect(status).toHaveTextContent('Analysing locally: 1 running, 2 waiting.');
    await user.click(screen.getByRole('button', { name: 'Pause' }));
    expect(lastReq(be, 'auto_settings_set')).toEqual({ enabled: true, paused: true, model: 'qwen3.5:9b' });
    expect(await screen.findByText('Paused. Nothing is analysed until you resume.')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Resume' }));
    expect(lastReq(be, 'auto_settings_set')).toMatchObject({ paused: false });
    await user.click(await screen.findByRole('button', { name: 'Stop waiting analyses' }));
    expect(be.calls.some((c) => c.cmd === 'auto_queue_cancel')).toBe(true);
    be.control.attention = [{ source_id: 'fixture-src-x', subject: 'Synthetic failing message', sender: null, label: 'FIXTURE',
      received_at: '2026-10-19T10:00:00Z', state: 'cancelled', detail: '' }];
    await user.click(await screen.findByRole('button', { name: 'Retry stopped messages' }));
    expect(lastReq(be, 'auto_queue_retry')).toEqual({ source_id: null });
    expect(screen.queryByText(/All .* have been analysed/)).not.toBeInTheDocument();
  });

  it('reports Ollama offline and invalid output as needing attention, never as all clear', async () => {
    const be = autoBackend();
    Object.assign(be.settings, { enabled: true, worker_state: 'offline', backoff_until: '2026-10-19T10:05:00Z' });
    be.control.queue = { queued: 1, running: 0, failed: 1, cancelled: 0 };
    be.control.attention = [{ source_id: 'fixture-src-bad', subject: 'Synthetic odd message', sender: null, label: 'FIXTURE',
      received_at: '2026-10-19T10:00:00Z', state: 'failed', detail: 'The model answer could not be used.' }];
    open(be);
    expect(await screen.findByRole('status', { name: 'Assistant status' })).toHaveTextContent('Ollama is not running on this computer');
    const att = screen.getByRole('list', { name: 'Needs attention' });
    expect(within(att).getByText(/analysis stopped after repeated problems/)).toBeInTheDocument();
    expect(within(att).getByRole('link', { name: 'Synthetic odd message' })).toHaveAttribute('href', '#/inbox?source=fixture-src-bad');
    expect(screen.queryByText(/have been analysed/)).not.toBeInTheDocument();
  });

  it('does not call partly read long messages all clear', async () => {
    const be = autoBackend();
    be.settings.enabled = true;
    be.control.attention = [{ source_id: 'fixture-src-long', subject: 'Synthetic long message', sender: null, label: 'FIXTURE',
      received_at: '2026-10-19T10:00:00Z', state: 'truncated', detail: 'Only the first part of this long message was read.' }];
    open(be);
    const status = await screen.findByRole('status', { name: 'Assistant status' });
    expect(status).toHaveTextContent('1 long message(s) were only partly read');
    expect(screen.queryByText(/have been analysed/)).not.toBeInTheDocument();
    expect(within(screen.getByRole('list', { name: 'Needs attention' })).getByText(/only the first part was read/)).toBeInTheDocument();
  });

  it('sends exactly the displayed selection with edits and explicit date decisions in one approval', async () => {
    const be = autoBackend();
    be.settings.enabled = true;
    be.control.actionable = [cand(1), cand(2), cand(3, { date_status: 'ambiguous', date_text: 'Freitag', due_date: null })];
    be.control.updates = [cand(9, { kind: 'update', title: 'Lecture moved to room 0.01', due_date: null, date_status: 'none' })];
    be.control.routine = [{ source_id: 'fixture-src-r', subject: 'Mensa newsletter', sender: null, label: 'FIXTURE',
      received_at: '2026-10-19T10:00:00Z', state: 'routine', detail: '' }];
    const user = open(be);
    const list = await screen.findByRole('list', { name: 'Suggestions' });
    const amb = within(list).getByRole('listitem', { name: 'Suggestion: Submit sheet 3' });
    expect(within(amb).getByRole('checkbox', { name: 'Include' })).not.toBeChecked();
    expect(within(amb).getByText(/The message says “Freitag”/)).toBeInTheDocument();
    expect(within(list).getAllByText(/date-only task, no time/)).toHaveLength(3);
    // Exclude sheet 2, edit sheet 1's title, resolve the unclear date and include sheet 3.
    await user.click(within(within(list).getByRole('listitem', { name: 'Suggestion: Submit sheet 2' })).getByRole('checkbox', { name: 'Include' }));
    const title = screen.getByRole('textbox', { name: 'Title for Submit sheet 1' });
    await user.clear(title);
    await user.type(title, 'Sheet 1 (my words)');
    fireEvent.change(within(amb).getByLabelText('Due date for Submit sheet 3'), { target: { value: '2026-10-23' } });
    await user.click(within(amb).getByRole('checkbox', { name: 'Include' }));
    expect(screen.getByRole('list', { name: 'Updates' })).toHaveTextContent('Lecture moved to room 0.01');
    expect(screen.getByText(/Nothing was deleted or marked read/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Approve 2 selected' }));
    expect(be.control.batches).toEqual([[
      { id: 'fixture-cand-1', payload_hash: 'fixture-hash-1', title: 'Sheet 1 (my words)' },
      { id: 'fixture-cand-3', payload_hash: 'fixture-hash-3', date_resolved: true, due_date: '2026-10-23' },
    ]]);
    expect(await screen.findByText('Created 2 task(s).')).toBeInTheDocument();
    expect(be.tasks.map((t) => t.title)).toEqual(['Sheet 1 (my words)', 'Submit sheet 3']);
    expect(be.calls.filter((c) => c.cmd === 'candidate_batch_approve')).toHaveLength(1);
  });

  it('shows per-item refusal when the backend creates nothing', async () => {
    const be = autoBackend();
    be.settings.enabled = true;
    be.control.actionable = [cand(1), cand(2)];
    be.control.refuse.set('fixture-cand-2', 'The original message changed or was deleted. No task was created.');
    const user = open(be);
    await user.click(await screen.findByRole('button', { name: 'Approve 2 selected' }));
    expect(await screen.findByText('Nothing was created. Fix or exclude the marked items and approve again.')).toBeInTheDocument();
    const s2 = screen.getByRole('listitem', { name: 'Suggestion: Submit sheet 2' });
    expect(within(s2).getByRole('alert')).toHaveTextContent('original message changed');
    const s1 = screen.getByRole('listitem', { name: 'Suggestion: Submit sheet 1' });
    expect(within(s1).getByRole('alert')).toHaveTextContent('other selected items need attention');
    expect(be.tasks).toHaveLength(0);
    expect(screen.queryByText(/Created \d task/)).not.toBeInTheDocument();
  });

  it('keeps edits through background refresh and does not auto-select newly arrived items', async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const be = autoBackend();
    be.settings.enabled = true;
    be.control.actionable = [cand(1), cand(2)];
    const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
    window.location.hash = '#/today';
    setTransport(be.transport);
    render(<App />);
    await user.click(within(await screen.findByRole('listitem', { name: 'Suggestion: Submit sheet 2' })).getByRole('checkbox', { name: 'Include' }));
    be.control.actionable = [cand(0), ...be.control.actionable];
    await act(async () => { vi.advanceTimersByTime(15000); });
    const fresh = await screen.findByRole('listitem', { name: 'Suggestion: Submit sheet 0' });
    expect(within(fresh).getByRole('checkbox', { name: 'Include' })).not.toBeChecked();
    expect(within(screen.getByRole('listitem', { name: 'Suggestion: Submit sheet 2' })).getByRole('checkbox', { name: 'Include' })).not.toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Approve 1 selected' }));
    expect(be.control.batches).toEqual([[{ id: 'fixture-cand-1', payload_hash: 'fixture-hash-1' }]]);
  });

  it('pages beyond the first 50 suggestions with real totals', async () => {
    const be = autoBackend();
    be.settings.enabled = true;
    be.control.actionable = Array.from({ length: 60 }, (_, i) => cand(i + 1));
    const user = open(be);
    expect(await screen.findByText('1–25 of 60')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Previous' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Next' }));
    await user.click(await screen.findByRole('button', { name: 'Next' }));
    expect(await screen.findByText('51–60 of 60')).toBeInTheDocument();
    expect(lastReq(be, 'auto_summary')).toMatchObject({ actionable_offset: 50, limit: 25 });
    expect(screen.getByRole('button', { name: 'Next' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Approve 10 selected' }));
    expect(be.control.batches[0].map((i) => i.id)).toEqual(Array.from({ length: 10 }, (_, i) => `fixture-cand-${i + 51}`));
  });

  it('refreshes Today immediately after a committed batch without navigation', async () => {
    const be = autoBackend(); be.settings.enabled = true; be.control.actionable = [cand(1)];
    const user = open(be);
    const taskReads = be.calls.filter((c) => c.cmd === 'task_list').length;
    await user.click(await screen.findByRole('button', { name: 'Approve 1 selected' }));
    const fromSources = await screen.findByRole('region', { name: 'Unscheduled tasks' });
    expect(await within(fromSources).findByText('Submit sheet 1')).toBeInTheDocument();
    expect(be.calls.filter((c) => c.cmd === 'task_list').length).toBeGreaterThan(taskReads);
    expect(window.location.hash).toBe('#/today');
  });

  it('pages important updates and routine messages independently without changing task selection', async () => {
    const be = autoBackend(); be.settings.enabled = true; be.control.actionable = [cand(1)];
    be.control.updates = Array.from({ length: 60 }, (_, i) => cand(100 + i, { kind: 'update', title: `Course update ${i + 1}` }));
    be.control.routine = Array.from({ length: 60 }, (_, i) => ({ source_id: `routine-${i}`, subject: `Routine sample ${i + 1}`,
      sender: null, label: 'FIXTURE', received_at: '2026-10-19T10:00:00Z', state: 'routine', detail: '' }));
    const user = open(be);
    expect(await screen.findByText('1–25 of 60 updates')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Next updates' }));
    await user.click(await screen.findByRole('button', { name: 'Next updates' }));
    expect(await screen.findByText('51–60 of 60 updates')).toBeInTheDocument();
    expect(within(screen.getByRole('list', { name: 'Updates' })).getByText(/Course update 60/)).toBeInTheDocument();
    await user.click(screen.getByText('Filed as routine (60)'));
    await user.click(screen.getByRole('button', { name: 'Next routine' }));
    await user.click(await screen.findByRole('button', { name: 'Next routine' }));
    expect(await screen.findByText('51–60 of 60 routine messages')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Routine sample 60' })).toBeInTheDocument();
    expect(lastReq(be, 'auto_summary')).toMatchObject({ actionable_offset: 0, updates_offset: 50, routine_offset: 50 });
    await user.click(screen.getByRole('button', { name: 'Approve 1 selected' }));
    expect(be.control.batches).toEqual([[{ id: 'fixture-cand-1', payload_hash: 'fixture-hash-1' }]]);
  });
});
