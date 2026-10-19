// P14 PWA tests. SYNTHETIC data. The AEAD is node:crypto's audited chacha20-poly1305 (same
// RFC 8439 construction as the Rust core) injected as the test provider.
import { describe, expect, it } from 'vitest';
import { PwaError, PwaSession, sealSnapshot, type Envelope, type Snapshot } from '../../../src/pwa/core';
import { hex, testAead as aead, unhex } from '../test-aead';

const enc = new TextEncoder();
const CODE = 'synthetic-pair-code';
const NOW = Date.parse('2026-10-19T10:00:00Z');
const iso = (ms: number) => new Date(ms).toISOString();

async function pairKey(code: string) {
  const base = await crypto.subtle.importKey('raw', enc.encode(code), 'HKDF', false, ['deriveBits']);
  return new Uint8Array(await crypto.subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: enc.encode('rei-os/v1'), info: enc.encode('pair') }, base, 256));
}

const epochKey = new Uint8Array(32).fill(7);
const task = (id: string, status: 'todo' | 'done' = 'todo', revision = 1) => ({
  id, revision, created_at: iso(NOW), updated_at: iso(NOW), origin_device_id: 'desk', deleted_at: null, title: `Task ${id}`, status,
});
const snap: Snapshot = { generated_at: iso(NOW), tasks: [task('t1')], events: [{ id: 'e1', title: 'Synthetic lecture' }], briefing: 'Synthetic briefing' };

async function offer(account = 'acct-1', secretExtra: object = {}): Promise<Envelope> {
  const secret = { account_id: account, device_id: 'pwa-1', epochs: { '1': hex(epochKey) }, current_epoch: 1, ...secretExtra };
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const ct = aead.seal(await pairKey(CODE), nonce, enc.encode(`rei-sync/v1|pair|${account}|pwa-1|0|offer`), enc.encode(JSON.stringify(secret)));
  return { op_id: 'pair-offer', account_id: account, device_id: 'pwa-1', epoch: 0, nonce: hex(nonce), ciphertext: hex(ct) };
}

async function ready() {
  const s = await PwaSession.pair(CODE, await offer(), aead);
  s.loadSnapshot(sealSnapshot(aead, epochKey, 'acct-1', 'desk', 1, 'snap-1', snap), NOW);
  return s;
}

const code = async (p: Promise<unknown> | (() => unknown)) => {
  try {
    await (typeof p === 'function' ? p() : p);
    return 'ok';
  } catch (e) {
    return e instanceof PwaError ? e.code : 'other';
  }
};

describe('P14 PWA companion', () => {
  it('pairs, decrypts a selected dataset and shows it', async () => {
    const s = await ready();
    expect(s.view()?.tasks).toHaveLength(1);
    expect(s.view()?.briefing).toBe('Synthetic briefing');
  });

  it('wrong pairing code yields no session', async () => {
    expect(await code(PwaSession.pair('wrong', await offer(), aead))).toBe('auth');
  });

  it('fails closed without an AEAD provider', async () => {
    expect(await code(PwaSession.pair(CODE, await offer(), null))).toBe('no_aead');
  });

  it('denies another account’s snapshot and ops', async () => {
    const s = await ready();
    const foreign = sealSnapshot(aead, epochKey, 'acct-2', 'desk', 1, 's', snap);
    expect(await code(() => s.loadSnapshot(foreign, NOW))).toBe('wrong_user');
    // re-attributing an envelope to our account breaks AAD authentication
    expect(await code(() => s.loadSnapshot({ ...foreign, account_id: 'acct-1' }, NOW))).toBe('auth');
  });

  it('rejects tampered ciphertext without changing data', async () => {
    const s = await ready();
    const env = sealSnapshot(aead, epochKey, 'acct-1', 'desk', 1, 'snap-2', snap);
    const flipped = { ...env, ciphertext: (env.ciphertext[0] === '0' ? '1' : '0') + env.ciphertext.slice(1) };
    expect(await code(() => s.loadSnapshot(flipped, NOW))).toBe('auth');
    expect(s.view()?.tasks).toHaveLength(1);
  });

  it('holds no privileged key: only content epoch keys from the offer, nothing in web storage', async () => {
    localStorage.clear();
    sessionStorage.clear();
    const s = await ready();
    s.quickAdd('x', iso(NOW));
    expect(localStorage.length + sessionStorage.length).toBe(0);
    expect(JSON.stringify(Object.keys(s))).not.toMatch(/service|role|token/i);
  });

  it('quick add and complete queue sealed ops that the desktop can open', async () => {
    const s = await ready();
    const added = s.quickAdd('  Buy stamps ', iso(NOW));
    s.complete('t1', iso(NOW));
    const ops = s.pendingOps();
    expect(ops).toHaveLength(2);
    expect(JSON.stringify(ops)).not.toContain('Buy stamps');
    const first = ops[0];
    const pt = JSON.parse(new TextDecoder().decode(aead.open(epochKey, unhex(first.nonce), enc.encode(`rei-sync/v1|op|acct-1|pwa-1|1|${first.op_id}`), unhex(first.ciphertext))));
    expect(pt).toMatchObject({ entity_type: 'task', entity_id: added.id, base_revision: 0, logical_seq: 1 });
    expect(pt.row.title).toBe('Buy stamps');
    expect(await code(() => s.quickAdd('   ', iso(NOW)))).toBe('shape');
    s.acknowledge([ops[0].op_id]);
    expect(s.pendingOps()).toHaveLength(1);
  });

  it('flags stale and never-synced data', async () => {
    const s = await ready();
    expect(s.freshness(NOW + 60_000).state).toBe('fresh');
    expect(s.freshness(NOW + 3_600_000).state).toBe('stale');
  });

  it('surfaces a conflict instead of overwriting a concurrent remote edit', async () => {
    const s = await ready();
    s.complete('t1', iso(NOW));
    const nonce = crypto.getRandomValues(new Uint8Array(12));
    const row = task('t1', 'todo', 5);
    const pt = enc.encode(JSON.stringify({ entity_type: 'task', entity_id: 't1', base_revision: 1, logical_seq: 1, row: { ...row, title: 'Renamed remotely' } }));
    const ct = aead.seal(epochKey, nonce, enc.encode('rei-sync/v1|op|acct-1|desk|1|r1'), pt);
    s.applyRemote({ op_id: 'r1', account_id: 'acct-1', device_id: 'desk', epoch: 1, nonce: hex(nonce), ciphertext: hex(ct) });
    expect(s.conflicts).toHaveLength(1);
    expect(s.view()?.tasks[0].status).toBe('done');
    expect(s.pendingOps()).toHaveLength(1);
  });

  it('lock refuses to drop unsent edits, then lock/logout wipe key and data', async () => {
    const s = await ready();
    s.quickAdd('keep me', iso(NOW));
    expect(await code(() => s.lock())).toBe('pending');
    expect(s.locked).toBe(false);
    s.lock(true);
    expect(s.locked).toBe(true);
    expect(await code(() => s.view())).toBe('locked');
    expect(await code(() => s.quickAdd('y', iso(NOW)))).toBe('locked');
    expect(s.pendingOps()).toHaveLength(0);
    const s2 = await ready();
    s2.logout();
    expect(s2.locked).toBe(true);
    expect(await code(() => s2.loadSnapshot(sealSnapshot(aead, epochKey, 'acct-1', 'desk', 1, 's3', snap), NOW))).toBe('locked');
  });
});
