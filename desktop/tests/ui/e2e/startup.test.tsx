import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { api, type AiSettings } from '../../../src/ipc';
import { Startup } from '../../../src/Startup';

const status = {ipc_version: 1, schema_version: 16, lock_state: 'unlocked', device_id: 'synthetic-device'};
const local: AiSettings = {revision: 1, provider: 'local', model: 'synthetic-model', local_model: 'synthetic-model', openai_saved: false, anthropic_saved: false, daily_limit: 50, used_today: 0, available_models: []};

beforeEach(() => {
  vi.spyOn(api, 'appStatus').mockResolvedValue(status);
  vi.spyOn(api, 'settingsGet').mockResolvedValue(null);
  vi.spyOn(api, 'aiSettings').mockResolvedValue(local);
  vi.spyOn(api, 'modelStatus').mockResolvedValue({state: 'ready', model: 'synthetic-model', busy: false});
});
afterEach(() => { vi.restoreAllMocks(); vi.useRealTimers(); });

it('keeps the planner behind a visible startup screen until the native workspace answers', async () => {
  let respond!: (value: typeof status) => void;
  vi.mocked(api.appStatus).mockImplementation(() => new Promise(resolve => { respond = resolve; }));
  const fallback = document.createElement('div'); fallback.id = 'rei-boot-fallback'; document.body.append(fallback);
  render(<Startup><main>Saved planner</main></Startup>);
  expect(screen.getByRole('status')).toHaveAttribute('aria-busy', 'true');
  expect(screen.queryByText('Saved planner')).toBeNull();
  expect(fallback.isConnected).toBe(false);
  await act(async () => { respond(status); });
  expect(await screen.findByText('Saved planner')).toBeInTheDocument();
  expect(api.modelStatus).toHaveBeenCalledWith('synthetic-model');
});

it('opens the planner with a useful notice when Ollama is not running, and allows dismissal', async () => {
  vi.mocked(api.modelStatus).mockResolvedValue({state: 'not_running', model: 'synthetic-model', busy: false});
  render(<Startup><main>Saved planner</main></Startup>);
  expect(await screen.findByText('Saved planner')).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('open Ollama');
  expect(screen.getByRole('link', {name: 'Open Settings'})).toHaveAttribute('href', '#/settings');
  fireEvent.click(screen.getByRole('button', {name: 'Dismiss startup notice'}));
  expect(screen.queryByRole('status')).toBeNull();
});

it('does not hold the planner indefinitely when the optional local AI check hangs', async () => {
  vi.useFakeTimers();
  vi.mocked(api.modelStatus).mockImplementation(() => new Promise(() => {}));
  render(<Startup><main>Saved planner</main></Startup>);
  await act(async () => { await vi.advanceTimersByTimeAsync(0); });
  expect(api.modelStatus).toHaveBeenCalled();
  expect(screen.queryByText('Saved planner')).toBeNull();
  await act(async () => { await vi.advanceTimersByTimeAsync(2200); });
  expect(screen.getByText('Saved planner')).toBeInTheDocument();
  expect(screen.getByRole('status')).toHaveTextContent('taking longer');
});

it('uses saved hosted AI settings without checking Ollama or invoking inference at startup', async () => {
  vi.mocked(api.aiSettings).mockResolvedValue({...local, provider: 'openai', model: 'synthetic-cloud-model', openai_saved: true});
  const summary = vi.spyOn(api, 'assistantSummaryRefresh');
  render(<Startup><main>Saved planner</main></Startup>);
  expect(await screen.findByText('Saved planner')).toBeInTheDocument();
  expect(api.modelStatus).not.toHaveBeenCalled();
  expect(summary).not.toHaveBeenCalled();
  expect(api.aiSettings).toHaveBeenCalledWith();
});

it('offers retry after a native workspace failure and does not erase or reset saved data', async () => {
  vi.mocked(api.appStatus).mockRejectedValueOnce(new Error('Synthetic native connection failure')).mockResolvedValue(status);
  const change = vi.spyOn(api, 'settingsSet');
  render(<Startup><main>Saved planner</main></Startup>);
  expect(await screen.findByRole('alert')).toHaveTextContent('saved data has not been reset');
  expect(screen.queryByText('Saved planner')).toBeNull();
  fireEvent.click(screen.getByRole('button', {name: 'Try again'}));
  await waitFor(() => expect(screen.getByText('Saved planner')).toBeInTheDocument());
  expect(change).not.toHaveBeenCalled();
});
