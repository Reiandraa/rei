import { NAV, DEFAULT_SCREEN } from '../../../src/nav';
import { COMMANDS } from '../../../src/ipc';

describe('navigation model', () => {
  it('has unique ids and includes the default screen', () => {
    const ids = NAV.map((n) => n.id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toContain(DEFAULT_SCREEN);
  });

  it('keeps everyday navigation together and Settings separate', () => {
    expect(NAV.filter(n=>n.group==='primary').map(n=>n.id)).toEqual(['today','tasks','calendar','inbox','assistant']);
    expect(NAV.filter(n=>n.group==='utility').map(n=>n.id)).toEqual(['settings']);
  });

  it('exposes no duplicate IPC commands', () => {
    expect(new Set(COMMANDS).size).toBe(COMMANDS.length);
  });
});
