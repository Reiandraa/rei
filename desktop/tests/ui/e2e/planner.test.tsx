// UI flow tests in jsdom against the SYNTHETIC backend (tests/ui/fake-backend.ts); the Rust logic
// itself is covered by src-tauri/tests/planner.rs.
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../../../src/App';
import { setTransport } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';

afterEach(() => setTransport(null));

describe('planner screens', () => {
  it('creates a task with confirmation, completes it and deletes it only after confirmation', async () => {
    const be = syntheticBackend();
    setTransport(be.transport);
    window.location.hash = '#/tasks';
    const user = userEvent.setup();
    render(<App />);
    expect(await screen.findByText(/No tasks yet/)).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '+ Add task' }));
    expect(screen.getByRole('button', { name: 'Add task' })).toBeDisabled();
    await user.type(screen.getByLabelText(/What needs to be done/), 'Write report');
    await user.click(screen.getByRole('button', { name: 'Add task' }));
    expect(await screen.findByText('Task saved.')).toBeInTheDocument();
    expect(await screen.findByText('Write report')).toBeInTheDocument();
    expect(screen.queryByRole('dialog')).toBeNull();
    expect(be.calls.find((c) => c.cmd === 'task_create')?.args).toEqual({ req: { title: 'Write report' } });

    await user.click(screen.getByRole('button', { name: 'Complete: Write report' }));
    await waitFor(() => expect(be.tasks[0].status).toBe('done'));
    expect(await screen.findByText(/No due date · Done/)).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Delete: Write report' }));
    expect(be.tasks).toHaveLength(1); // first click only asks for confirmation
    await user.click(screen.getByRole('button', { name: 'Confirm delete: Write report' }));
    await waitFor(() => expect(be.tasks).toHaveLength(0));
    expect(await screen.findByText(/No tasks yet/)).toBeInTheDocument();
  });

  it('keeps advanced repeat options hidden until opened and sends the rule with the local time zone', async () => {
    const be = syntheticBackend();
    setTransport(be.transport);
    window.location.hash = '#/tasks';
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole('button', { name: '+ Add task' }));
    await user.type(await screen.findByLabelText(/What needs to be done/), 'Lecture');
    expect(screen.getByText('More options').closest('details')).not.toHaveAttribute('open');
    await user.click(screen.getByText('More options'));
    await user.selectOptions(screen.getByLabelText('Repeat'), 'WEEKLY');
    await user.click(screen.getByRole('button', { name: 'Add task' }));
    await waitFor(() => expect(be.tasks).toHaveLength(1));
    const rec = (be.calls.find((c) => c.cmd === 'task_create')!.args as { req: { recurrence: Record<string, string> } }).req.recurrence;
    expect(rec.rrule).toMatch(/^FREQ=WEEKLY;BYDAY=(MO|TU|WE|TH|FR|SA|SU)$/);
    expect(rec.dtstart_local).toMatch(/^\d{4}-\d{2}-\d{2}T09:00:00$/);
    expect(rec.tz).toBeTruthy();
    expect(await screen.findByText(/Repeats every week/)).toBeInTheDocument();
  });

  it('shows a refusal instead of a saved confirmation when task creation fails', async () => {
    const be = syntheticBackend();
    const inner = be.transport;
    setTransport(async (cmd, args) => {
      if (cmd === 'task_create') throw { code: 'Validation', message: 'title is too long' };
      return inner(cmd, args);
    });
    window.location.hash = '#/tasks';
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole('button', { name: '+ Add task' }));
    await user.type(await screen.findByLabelText(/What needs to be done/), 'X');
    await user.click(screen.getByRole('button', { name: 'Add task' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('title is too long');
    expect(screen.queryByText(/Task saved/)).toBeNull();
  });

  it('shows agenda overlaps and applies a per-occurrence skip on Today', async () => {
    const be = syntheticBackend();
    const day = new Date();
    const iso = `${day.getFullYear()}-${String(day.getMonth() + 1).padStart(2, '0')}-${String(day.getDate()).padStart(2, '0')}`;
    be.agenda.push({
      kind: 'occurrence', id: 't1', title: 'Lecture', local_start: `${iso}T09:00:00`, all_day: false, state: 'Normal',
      original_local: `${iso}T09:00:00`, task_revision: 3, recurring: true, read_only: false, conflict: true,
    });
    setTransport(be.transport);
    const user = userEvent.setup();
    render(<App />);
    expect(await screen.findByText(/Overlaps another item/)).toBeInTheDocument();
    await user.click(screen.getByLabelText('More options: Lecture'));
    await user.click(screen.getByRole('button', { name: 'Skip this time: Lecture' }));
    const call = be.calls.find((c) => c.cmd === 'occurrence_edit')!;
    expect(call.args).toEqual({
      req: { task_id: 't1', expected_revision: 3, scope: 'this', action: 'skip', original_local: `${iso}T09:00:00` },
    });
    expect(await screen.findByRole('button', { name: 'Undo: Lecture' })).toBeInTheDocument();
  });

  it('approves with the exact hash and keeps the current value on the other proposal', async () => {
    const be = syntheticBackend();
    be.tasks.push(
      { id: 't1', revision: 1, title: 'Form', status: 'todo', due_date: '2026-10-10', due_at_local: null, recurrence: null },
      { id: 't2', revision: 1, title: 'Letter', status: 'todo', due_date: '2026-10-11', due_at_local: null, recurrence: null },
    );
    const base = {
      action: 'task.reschedule', expected_target_revision: 1,
      payload: { due: { kind: 'date', date: '2026-10-19' } }, expires_at: '2026-10-20T08:00:00Z',
      status: 'Pending', confidence: 0.9, rationale: 'It is overdue.', created_at: 'x',
    };
    be.proposals.push(
      { ...base, id: 'p1', target_id: 't1', target_title: 'Form', payload_hash: 'h1' },
      { ...base, id: 'p2', target_id: 't2', target_title: 'Letter', payload_hash: 'h2' },
    );
    setTransport(be.transport);
    window.location.hash = '#/proposals';
    const user = userEvent.setup();
    render(<App />);
    expect(await screen.findByText(/Nothing changes unless you approve it/)).toBeInTheDocument();
    expect(await screen.findAllByText('Monday, 19 October 2026')).toHaveLength(2); // proposed value
    expect(screen.getByText('Saturday, 10 October 2026')).toBeInTheDocument(); // current value
    expect(screen.getAllByText(/Why this was suggested/)).toHaveLength(2);
    expect(document.body.textContent).not.toMatch(/h1|h2|payload|t1\b|p1\b/);
    await user.click(screen.getByRole('button', { name: 'Approve: Form' }));
    await waitFor(() => expect(be.proposals[0].status).toBe('Executed'));
    expect(be.calls.find((c) => c.cmd === 'proposal_approve')?.args).toEqual({ req: { id: 'p1', payload_hash: 'h1' } });
    await user.click(await screen.findByRole('button', { name: 'Keep current: Letter' }));
    await waitFor(() => expect(be.proposals[1].status).toBe('Rejected'));
    expect(await screen.findByText(/No changes are waiting for your decision/)).toBeInTheDocument();
    expect(be.proposals[0].status).toBe('Executed');
    expect(screen.getByText('Approved and applied')).toBeInTheDocument();
    expect(screen.getByText('Kept current')).toBeInTheDocument();
  });

  it('reports a backend refusal instead of pretending success', async () => {
    const be = syntheticBackend();
    be.proposals.push({ id: 'p1', target_title: 'Form', payload_hash: 'h1', status: 'Pending', payload: {}, action: 'x', rationale: '', expires_at: 'e' });
    const inner = be.transport;
    setTransport(async (cmd, args) => {
      if (cmd === 'proposal_approve') throw { code: 'ApprovalInvalid', message: 'target changed since the proposal was made' };
      return inner(cmd, args);
    });
    window.location.hash = '#/proposals';
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole('button', { name: 'Approve: Form' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('target changed since the proposal was made');
  });
});
