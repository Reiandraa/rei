// jsdom + SYNTHETIC IPC for the Connections panel. No HIZ/Teams/OS vault is contacted;
// the fake records which real Rust commands each control would invoke.
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ConnectionsPanel } from '../../../src/screens/Connections';
import { setTransport, type ProviderAccount, type Transport } from '../../../src/ipc';

afterEach(() => setTransport(null));

function backend() {
  const calls: { cmd: string; req: Record<string, unknown> }[] = [];
  const accounts: ProviderAccount[] = [];
  let polling = { enabled: false, interval_minutes: 30 };
  const acct = (p: Partial<ProviderAccount>): ProviderAccount => ({
    account_id: 'a1', provider: 'hiz_imap', kind: 'email', display_name: 'HIZ mailbox', state: 'consent_needed', evidence: 'none',
    settings_saved: true, sign_in_saved: false, authenticated_at: null, last_read_ok_at: null, last_attempt_at: null,
    last_success_at: null, last_outcome: null, last_error: null, config: {}, sources: [], stored_messages: 0, fetch_running: false, ...p });
  const transport: Transport = async (cmd, args) => {
    const req = (args?.req ?? {}) as Record<string, unknown>;
    calls.push({ cmd, req });
    switch (cmd) {
      case 'connections_status': return { accounts: structuredClone(accounts), polling, hiz_host: 'mail.hiz-saarland.de', hiz_port: 993, os_vault_available: true };
      case 'hiz_configure': accounts.push(acct({})); return 'a1';
      case 'hiz_sign_in':
        if (req.password !== 'SYNTHETIC-pw') throw { code: 'ProviderAuthRequired', message: 'The provider rejected the sign-in.' };
        Object.assign(accounts[0], { sign_in_saved: true, sources: [
          { source_kind: 'folder', source_ref: 'INBOX', display_name: 'INBOX', selected: false },
          { source_kind: 'folder', source_ref: 'Moodle', display_name: 'Moodle', selected: false }] });
        return ['INBOX', 'Moodle'];
      case 'connection_select_sources':
        for (const s of accounts[0].sources) s.selected = (req.selected as { source_ref: string }[]).some((x) => x.source_ref === s.source_ref);
        return 1;
      case 'connection_fetch_now':
        Object.assign(accounts[0], { state: 'testing', evidence: 'fixture', last_read_ok_at: '2026-10-19T10:00:00Z', stored_messages: 2,
          last_outcome: 'Read 1 folder(s): 2 new, 0 updated, 0 removed' });
        return { inserted: 2 };
      case 'connection_disconnect': Object.assign(accounts[0], { state: 'not_configured', sign_in_saved: false }); return true;
      case 'teams_configure': accounts.push(acct({ account_id: 't1', provider: 'teams_graph', kind: 'teams_chat', display_name: 'Teams chats' })); return 't1';
      case 'teams_sign_in': throw { code: 'ProviderDenied', message: 'The provider refused access (403). The tenant may require admin consent; this is not confirmed as a denial.' };
      case 'polling_set': polling = { enabled: req.enabled as boolean, interval_minutes: req.interval_minutes as number }; return polling;
      default: throw new Error(`unexpected ${cmd}`);
    }
  };
  return { calls, transport };
}

describe('connections panel (synthetic transport)', () => {
  it('sets up HIZ, signs in once, selects a folder, fetches and never shows fixture data as connected', async () => {
    const be = backend();
    setTransport(be.transport);
    const user = userEvent.setup();
    render(<ConnectionsPanel />);
    expect(await screen.findByText(/mail.hiz-saarland.de:993, IMAP over TLS/)).toBeInTheDocument();
    await user.type(screen.getByLabelText('HIZ identifier'), 's8synth');
    await user.click(screen.getByRole('button', { name: 'Save HIZ settings' }));
    const card = await screen.findByRole('article', { name: 'Connection: HIZ mailbox' });
    expect(within(card).getByText('Settings saved — sign-in needed')).toBeInTheDocument();
    expect(within(card).getByRole('button', { name: 'Fetch now' })).toBeDisabled();
    await user.type(within(card).getByLabelText('HIZ password'), 'wrong');
    await user.click(within(card).getByRole('button', { name: 'Sign in and list folders' }));
    expect(await screen.findByText('The provider rejected the sign-in.')).toBeInTheDocument();
    const c2 = await screen.findByRole('article', { name: 'Connection: HIZ mailbox' });
    expect(within(c2).getByLabelText('HIZ password')).toHaveValue('');
    await user.type(within(c2).getByLabelText('HIZ password'), 'SYNTHETIC-pw');
    await user.click(within(c2).getByRole('button', { name: 'Sign in and list folders' }));
    const c3 = await screen.findByRole('article', { name: 'Connection: HIZ mailbox' });
    await waitFor(() => expect(within(c3).getByLabelText('INBOX')).toBeInTheDocument());
    expect(within(c3).getByLabelText('Moodle')).not.toBeChecked();
    await user.click(within(c3).getByLabelText('INBOX'));
    await user.click(within(c3).getByRole('button', { name: 'Save selection' }));
    const c4 = await screen.findByRole('article', { name: 'Connection: HIZ mailbox' });
    await waitFor(() => expect(within(c4).getByRole('button', { name: 'Fetch now' })).toBeEnabled());
    await user.click(within(c4).getByRole('button', { name: 'Fetch now' }));
    expect(await screen.findByText('Synthetic test data only — not a connected account')).toBeInTheDocument();
    expect(screen.queryByText(/Connected \(a real read/)).toBeNull();
    const sel = be.calls.find((c) => c.cmd === 'connection_select_sources')!;
    expect(sel.req.selected).toEqual([{ source_kind: 'folder', source_ref: 'INBOX' }]);
    // The password only ever went to hiz_sign_in, never to another command.
    expect(be.calls.filter((c) => JSON.stringify(c.req).includes('SYNTHETIC-pw')).map((c) => c.cmd)).toEqual(['hiz_sign_in']);
  });

  it('requires an explicit purge choice on disconnect', async () => {
    const be = backend();
    setTransport(be.transport);
    const user = userEvent.setup();
    render(<ConnectionsPanel />);
    await user.type(await screen.findByLabelText('HIZ identifier'), 's8synth');
    await user.click(screen.getByRole('button', { name: 'Save HIZ settings' }));
    await user.click(await screen.findByRole('button', { name: 'Disconnect…' }));
    const g = screen.getByRole('group', { name: 'Confirm disconnect' });
    await user.click(within(g).getByRole('button', { name: 'Disconnect, keep messages' }));
    expect(await screen.findByText('Disconnected; messages kept.')).toBeInTheDocument();
    expect(be.calls.find((c) => c.cmd === 'connection_disconnect')!.req).toEqual({ account_id: 'a1', purge: false });
  });

  it('Teams setup shows owner app-registration needs and consent is not shown as a denial', async () => {
    const be = backend();
    setTransport(be.transport);
    const user = userEvent.setup();
    render(<ConnectionsPanel />);
    expect(await screen.findByText(/Needs an application \(client\) ID/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save Teams settings' })).toBeDisabled();
    await user.type(screen.getByLabelText('Application (client) ID'), '11111111-2222-3333-4444-555555555555');
    await user.click(screen.getByLabelText('Also read selected team channels'));
    await user.click(screen.getByRole('button', { name: 'Save Teams settings' }));
    expect(be.calls.find((c) => c.cmd === 'teams_configure')!.req).toMatchObject({ tenant: 'organizations', channels: true });
    await user.click(await screen.findByRole('button', { name: 'Sign in with Microsoft (opens your browser)' }));
    expect(await screen.findByText(/not confirmed as a denial/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'List my chats and channels' })).toBeDisabled();
  });

  it('polling is opt-in, bounded and explains it only runs while the app is open', async () => {
    const be = backend();
    setTransport(be.transport);
    const user = userEvent.setup();
    render(<ConnectionsPanel />);
    expect(await screen.findByText(/Only while REI OS is open and the computer is awake/)).toBeInTheDocument();
    expect(screen.getByLabelText('Fetch automatically')).not.toBeChecked();
    await user.click(screen.getByLabelText('Fetch automatically'));
    await user.click(screen.getByRole('button', { name: 'Save schedule' }));
    expect(await screen.findByText('Automatic fetching on.')).toBeInTheDocument();
    expect(be.calls.find((c) => c.cmd === 'polling_set')!.req).toEqual({ enabled: true, interval_minutes: 30 });
  });

  it('refreshes running status and permits cancellation before a long sign-in returns', async () => {
    const be = backend();
    await be.transport('hiz_configure', { req: { username: 'synthetic', initial_count: 10 } });
    let busy = false;
    let rejectPending!: (reason: unknown) => void;
    const pending = new Promise<unknown>((_, reject) => { rejectPending = reject; });
    let cancellations = 0;
    const transport: Transport = async (cmd, args) => {
      if (cmd === 'hiz_sign_in') { busy = true; return pending; }
      if (cmd === 'connection_fetch_cancel') {
        cancellations++; busy = false;
        rejectPending({ code: 'Cancelled', message: 'Cancelled by you.' });
        return true;
      }
      const result = await be.transport(cmd, args);
      if (cmd === 'connections_status') {
        const value = result as { accounts: ProviderAccount[] };
        value.accounts[0].fetch_running = busy;
      }
      return result;
    };
    setTransport(transport);
    const user = userEvent.setup();
    render(<ConnectionsPanel />);
    await user.type(await screen.findByLabelText('HIZ password'), 'SYNTHETIC-pw');
    await user.click(screen.getByRole('button', { name: 'Sign in and list folders' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Cancel fetch' })).toBeEnabled(), { timeout: 2500 });
    await user.click(screen.getByRole('button', { name: 'Cancel fetch' }));
    await waitFor(() => expect(cancellations).toBe(1));
    expect(screen.getByLabelText('HIZ password')).toHaveValue('');
  });
});
