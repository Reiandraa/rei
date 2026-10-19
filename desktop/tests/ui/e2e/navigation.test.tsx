// UI flow tests in jsdom (not a real browser or Tauri window; see TEST-REPORT.md).
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../../../src/App';
import { NAV } from '../../../src/nav';
import { setTransport } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';
import { s } from '../../../src/strings';

afterEach(() => {
  setTransport(null);
  document.documentElement.removeAttribute('data-theme');
});

describe('navigation', () => {
  it('shows five everyday pages in order, Settings below, and no Workspaces or review navigation', () => {
    setTransport(syntheticBackend().transport);render(<App/>);
    const nav=screen.getByRole('navigation',{name:'Main navigation'});
    expect(within(nav).getAllByRole('link').map(a=>a.textContent)).toEqual(['Today','Tasks','Calendar','Messages','Assistant','Settings']);
    expect(within(nav).queryByText('Workspaces')).toBeNull();expect(within(nav).queryByText('Review changes')).toBeNull();
  });

  it('reaches every screen and marks the current page', async () => {
    setTransport(syntheticBackend().transport);
    const user = userEvent.setup();
    render(<App />);
    const nav = screen.getByRole('navigation', { name: 'Main navigation' });
    for (const item of NAV.filter(n=>n.group!=='internal')) {
      await user.click(within(nav).getByRole('link', { name: item.label }));
      expect(await screen.findByRole('heading', { level: 1, name: item.label })).toBeInTheDocument();
      expect(within(nav).getByRole('link', { name: item.label })).toHaveAttribute('aria-current', 'page');
    }
  });

  it('opens Assistant without starting inference just by navigating there', async () => {
    const be=syntheticBackend();setTransport(be.transport);window.location.hash='#/assistant';render(<App/>);
    expect(await screen.findByText('Choose Summarize to prepare your important-message briefing.')).toBeInTheDocument();
    expect(be.calls.some(c=>c.cmd==='assistant_summary_refresh')).toBe(false);
  });

  it('says plainly that sync is unavailable and sensitive data stays local', async () => {
    window.location.hash = '#/sync';
    setTransport(syntheticBackend().transport);render(<App />);
    expect(await screen.findByText(/Your messages, tasks and preferences stay on this computer/)).toBeInTheDocument();
  });

  it('falls back to the default screen on unknown routes', () => {
    window.location.hash = '#/nope';
    render(<App />);
    expect(screen.getByRole('heading', { level: 1 })).toHaveTextContent('Today');
  });
});

describe('settings', () => {
  it('truthfully shows English as the only language and has no language selector', async () => {
    setTransport(syntheticBackend().transport);
    window.location.hash = '#/settings?tab=appearance';
    render(<App />);
    expect(await screen.findByText(/Language · English/)).toBeInTheDocument();
    expect(screen.queryByRole('combobox', { name: /language/i })).toBeNull();
  });

  it('changes the interface theme, persists it with its revision and restores it on reload', async () => {
    const b = syntheticBackend();
    setTransport(b.transport);
    window.location.hash = '#/settings?tab=appearance';
    const user = userEvent.setup();
    const first = render(<App />);
    await screen.findByRole('combobox',{name:'Appearance'});
    await user.selectOptions(screen.getByRole('combobox',{name:'Appearance'}), 'dark');
    expect(await screen.findByText('Appearance saved: Dark.')).toBeInTheDocument();
    expect(document.documentElement).toHaveAttribute('data-theme', 'dark');
    expect(b.store.get('theme')).toEqual({ value: 'dark', revision: 1 });
    await user.selectOptions(screen.getByRole('combobox',{name:'Appearance'}), 'light');
    expect(document.documentElement).toHaveAttribute('data-theme', 'light');
    expect(b.store.get('theme')).toEqual({ value: 'light', revision: 2 });
    expect(within(screen.getByRole('combobox',{name:'Appearance'})).getAllByRole('option').map(o=>o.textContent)).toEqual(['Light','Dark']);

    // Restart: the saved value (light) is read from the backend and applied at start-up on any screen.
    await user.selectOptions(screen.getByRole('combobox',{name:'Appearance'}), 'light');
    first.unmount();
    document.documentElement.removeAttribute('data-theme');
    window.location.hash = '#/tasks';
    render(<App />);
    await screen.findByRole('heading', { name: 'Tasks', level: 1 });
    await vi.waitFor(() => expect(document.documentElement).toHaveAttribute('data-theme', 'light'));
  });

  it('does not apply a theme the backend refused', async () => {
    const b = syntheticBackend();
    const inner = b.transport;
    setTransport(async (cmd, args) => {
      if (cmd === 'settings_set') throw { code: 'Internal', message: 'disk full' };
      return inner(cmd, args);
    });
    window.location.hash = '#/settings?tab=appearance';
    const user = userEvent.setup();
    render(<App />);
    await user.selectOptions(await screen.findByRole('combobox',{name:'Appearance'}), 'dark');
    expect(await within(await screen.findByRole('region', { name: s.settings.controls })).findByRole('alert')).toHaveTextContent('disk full');
    expect(document.documentElement).toHaveAttribute('data-theme','light');
  });

  it('keeps technical diagnostics in a collapsed Diagnostics section', async () => {
    setTransport(syntheticBackend().transport);
    window.location.hash = '#/settings?tab=appearance';
    const user = userEvent.setup();
    render(<App />);
    await user.click(await screen.findByRole('tab',{name:'Advanced'}));
    const summary = await screen.findByText('Diagnostics');
    const details = summary.closest('details')!;
    expect(details).not.toHaveAttribute('open');
    expect(within(details).getByText('synthetic-device')).toBeInTheDocument();
    await user.click(summary);
    expect(details).toHaveAttribute('open');
    expect(within(details).getByText('Working (schema version 1)')).toBeInTheDocument();
  });

  it('shows an error with retry when the backend is unavailable', async () => {
    window.location.hash = '#/settings?tab=appearance';
    render(<App />);
    expect(await screen.findByRole('alert')).toHaveTextContent('Desktop backend not available');
    expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled();
  });

  it('reloads on a revision conflict instead of reporting success', async () => {
    const b = syntheticBackend();
    setTransport(b.transport);
    window.location.hash = '#/settings?tab=appearance';
    const user = userEvent.setup();
    render(<App />);
    await screen.findByRole('combobox',{name:'Appearance'});
    b.store.set('theme', { value: 'light', revision: 5 }); // changed elsewhere
    await user.selectOptions(screen.getByRole('combobox',{name:'Appearance'}), 'dark');
    expect(await within(await screen.findByRole('region', { name: s.settings.controls })).findByRole('alert')).toHaveTextContent('changed somewhere else');
    expect(await screen.findByRole('combobox',{name:'Appearance'})).toHaveValue('light');
    expect(screen.queryByText(/Appearance saved/)).toBeNull();
  });
});
