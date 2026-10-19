import { api, asIpcError, setTransport } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';

afterEach(() => setTransport(null));

describe('ipc client', () => {
  it('reports Unavailable outside Tauri', async () => {
    setTransport(null);
    await expect(api.appStatus()).rejects.toMatchObject({ code: 'Unavailable' });
  });

  it('sends the request envelope expected by the Rust commands', async () => {
    const b = syntheticBackend();
    setTransport(b.transport);
    await api.settingsSet('theme', 'dark', 0);
    expect(b.calls.at(-1)).toEqual({
      cmd: 'settings_set',
      args: { req: { key: 'theme', value: 'dark', expected_revision: 0 } },
    });
  });

  it('normalizes unknown errors', () => {
    expect(asIpcError(new Error('x'))).toMatchObject({ code: 'Internal' });
    expect(asIpcError({ code: 'Conflict', message: 'm' })).toMatchObject({ code: 'Conflict' });
  });
});
