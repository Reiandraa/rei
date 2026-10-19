// English daily-workflow test in jsdom against the SYNTHETIC in-memory backend (tests/ui/fake-backend.ts).
// NOT a real browser, native window or the Rust backend. Written for scripts/tools/ui-flow-evidence.mjs.
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../../../src/App';
import { setTransport } from '../../../src/ipc';
import { isoDate } from '../../../src/screens/Planner';
import { syntheticBackend } from '../fake-backend';

afterEach(() => setTransport(null));

const plusDays = (n: number) => {
  const d = new Date();
  return isoDate(new Date(d.getFullYear(), d.getMonth(), d.getDate() + n));
};

describe('ui-flow: Today → add task → see it scheduled → complete → review changes → restart', () => {
  it('walks the whole daily workflow and keeps its data after a restart', async () => {
    const be = syntheticBackend();
    setTransport(be.transport);
    const user = userEvent.setup();
    const session = render(<App />);

    // Today: readable date, explained empty states, and obvious next steps.
    expect(screen.getByRole('heading', { level: 1, name: 'Today' })).toBeInTheDocument();
    expect(await screen.findByText(/^(Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday), \d{1,2} \w+ \d{4}$/)).toBeInTheDocument();
    expect(await screen.findByText('Nothing planned yet. Choose a task below and give it some time.')).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Planned tasks' })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Open calendar' })).toHaveAttribute('href', '#/calendar');
    await user.click(screen.getByRole('link', { name: 'Tasks' }));

    // Tasks: one undated task, one for today, one for later.
    for (const [title, due] of [['Call the bank', ''], ['Submit form', plusDays(0)], ['Plan trip', plusDays(3)]]) {
      await user.click(await screen.findByRole('button', { name: '+ Add task' }));
      const form = screen.getByRole('form', { name: 'New task' });
      await user.type(within(form).getByLabelText(/What needs to be done/), title);
      if (due) await user.type(within(form).getByLabelText(/Due date/), due);
      await user.click(within(form).getByRole('button', { name: 'Add task' }));
      expect(await screen.findByText('Task saved.')).toBeInTheDocument();
    }
    expect(be.calls.filter((c) => c.cmd === 'task_create').map((c) => c.args)).toEqual([
      { req: { title: 'Call the bank' } },
      { req: { title: 'Submit form', due: { kind: 'date', date: plusDays(0) } } },
      { req: { title: 'Plan trip', due: { kind: 'date', date: plusDays(3) } } },
    ]);

    // Today again: items are split between today, upcoming and undated, with real counts.
    await user.click(screen.getByRole('link', { name: 'Today' }));
    const todaySection = await screen.findByRole('region', { name: 'Deadlines' });
    expect(await within(todaySection).findByText('Submit form')).toBeInTheDocument();
    await user.click(screen.getByText('Upcoming tasks · next 6 days'));
    const upcomingSection = screen.getByText('Upcoming tasks · next 6 days').closest('details')!;
    expect(within(upcomingSection).getByText('Plan trip')).toBeInTheDocument();
    expect(within(upcomingSection).queryByText('Submit form')).toBeNull();
    const undated = screen.getByRole('region', { name: 'Unscheduled tasks' });
    expect(within(undated).getByText('Call the bank')).toBeInTheDocument();
    expect(within(todaySection).getByText('Submit form')).toBeInTheDocument(); // A deadline alone never becomes a work plan.
    expect(within(screen.getByRole('region', { name: 'Planned tasks' })).queryByText('Submit form')).toBeNull();

    // Complete the undated task on Tasks; it leaves the "without a date" list.
    await user.click(screen.getByRole('link', { name: 'Tasks' }));
    await user.click(await screen.findByRole('button', { name: 'Complete: Call the bank' }));
    await waitFor(() => expect(be.tasks.find((t) => t.title === 'Call the bank')!.status).toBe('done'));

    // Review changes: a synthetic overdue proposal, explained and approved with the exact hash.
    be.tasks.push({ id: 'old', revision: 1, title: 'Renew pass', status: 'todo', due_date: plusDays(-5), due_at_local: null, recurrence: null });
    be.proposals.push({
      id: 'p1', action: 'task.reschedule', target_id: 'old', target_title: 'Renew pass', expected_target_revision: 1,
      payload: { due: { kind: 'date', date: plusDays(1) } }, payload_hash: 'hash-1', expires_at: `${plusDays(2)}T08:00:00Z`,
      status: 'Pending', confidence: 0.8, rationale: 'It is five days overdue.', created_at: 'x',
    });
    await user.click(screen.getByRole('link',{name:'Today'}));
    await user.click(await screen.findByText('Other proposed changes'));
    expect(await screen.findByText(/It is five days overdue/)).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/hash-1|payload/);
    await user.click(screen.getByRole('button', { name: 'Approve: Renew pass' }));
    await waitFor(() => expect(be.proposals[0].status).toBe('Executed'));
    expect(be.calls.find((c) => c.cmd === 'proposal_approve')?.args).toEqual({ req: { id: 'p1', payload_hash: 'hash-1' } });
    expect(await screen.findByText('Approved and applied')).toBeInTheDocument();

    // Restart: a fresh app instance on the same backend shows the same saved data.
    session.unmount();
    window.location.hash = '#/tasks';
    render(<App />);
    expect(await screen.findByText('Submit form')).toBeInTheDocument();
    expect(screen.getByText('Plan trip')).toBeInTheDocument();
    expect(screen.getByText(/No due date · Done/)).toBeInTheDocument(); // Call the bank stayed completed
    expect(be.tasks.map((t) => t.title)).toEqual(['Call the bank', 'Submit form', 'Plan trip', 'Renew pass']);
  });

  it('shows no sample data on a fresh install', async () => {
    setTransport(syntheticBackend().transport);
    render(<App />);
    expect(await screen.findByText('No open tasks waiting for a work plan.')).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Planned tasks' })).toBeInTheDocument();
  });
});
