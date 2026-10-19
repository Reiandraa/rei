// P14 companion core. Memory-first: keys and decrypted data live only in a PwaSession object;
// nothing is written to web storage. No privileged/service key exists here: the session holds only
// the account's per-epoch content keys delivered by pairing. Wire format matches src-tauri/src/sync.rs
// (IETF ChaCha20-Poly1305, AAD "rei-sync/v1|kind|account|device|epoch|id", hex fields).
// The AEAD primitive is injected: WebCrypto has no ChaCha20, and the approved browser library
// (@noble/ciphers, ADR-013) is not installed yet (see BUILD-BLOCKERS.md). No home-grown cipher here.

export interface Aead {
  seal(key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, pt: Uint8Array): Uint8Array;
  open(key: Uint8Array, nonce: Uint8Array, aad: Uint8Array, sealed: Uint8Array): Uint8Array;
}

export interface Envelope {
  op_id: string;
  account_id: string;
  device_id: string;
  epoch: number;
  nonce: string;
  ciphertext: string;
}

export interface TaskRow {
  id: string;
  revision: number;
  created_at: string;
  updated_at: string;
  origin_device_id: string;
  deleted_at: string | null;
  title: string;
  status: 'todo' | 'doing' | 'done' | 'cancelled';
  [k: string]: unknown;
}

export interface EventRow {
  id: string;
  title: string;
  starts_at?: string;
  [k: string]: unknown;
}

export interface Snapshot {
  generated_at: string;
  tasks: TaskRow[];
  events: EventRow[];
  briefing: string | null;
}

export interface PwaConflict {
  entity_id: string;
  local_status: string;
  remote_revision: number;
  base_revision: number;
}

export type PwaErrorCode = 'locked' | 'auth' | 'wrong_user' | 'shape' | 'pending' | 'no_aead' | 'unknown_epoch';

export class PwaError extends Error {
  constructor(public code: PwaErrorCode) {
    super(code);
  }
}

const enc = new TextEncoder();
const dec = new TextDecoder();
const toHex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
const fromHex = (s: string) => {
  if (!/^([0-9a-f]{2})*$/.test(s)) throw new PwaError('shape');
  return Uint8Array.from(s.match(/../g) ?? [], (h) => parseInt(h, 16));
};
const aad = (kind: string, account: string, device: string, epoch: number, id: string) =>
  enc.encode(`rei-sync/v1|${kind}|${account}|${device}|${epoch}|${id}`);

/** Mirrors crypto::derive_key (HKDF-SHA256, salt "rei-os/v1"). */
async function deriveKey(ikm: Uint8Array, info: string): Promise<Uint8Array> {
  const base = await crypto.subtle.importKey('raw', ikm as BufferSource, 'HKDF', false, ['deriveBits']);
  const bits = await crypto.subtle.deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt: enc.encode('rei-os/v1'), info: enc.encode(info) },
    base,
    256,
  );
  return new Uint8Array(bits);
}

interface PairedSecret {
  account_id: string;
  device_id: string;
  epochs: Record<string, string>;
  current_epoch: number;
}

export type Freshness = { state: 'never' } | { state: 'fresh' | 'stale'; ageMs: number };

export class PwaSession {
  private keys: Map<number, Uint8Array> | null;
  private data: Snapshot | null = null;
  private syncedAt: number | null = null;
  private seq = 0;
  private outbox: Envelope[] = [];
  private pendingBase = new Map<string, { base: number; status: string }>();
  conflicts: PwaConflict[] = [];

  private constructor(
    private aead: Aead,
    readonly accountId: string,
    readonly deviceId: string,
    keys: Map<number, Uint8Array>,
    private epoch: number,
  ) {
    this.keys = keys;
  }

  /** Pairing: the code typed from the desktop decrypts the offer; a wrong code authenticates nothing. */
  static async pair(code: string, blob: Envelope, aead: Aead | null): Promise<PwaSession> {
    if (!aead) throw new PwaError('no_aead');
    const k = await deriveKey(enc.encode(code.trim()), 'pair');
    let secret: PairedSecret;
    try {
      const pt = aead.open(k, fromHex(blob.nonce), aad('pair', blob.account_id, blob.device_id, 0, 'offer'), fromHex(blob.ciphertext));
      secret = JSON.parse(dec.decode(pt)) as PairedSecret;
    } catch {
      throw new PwaError('auth');
    }
    if (secret.account_id !== blob.account_id || secret.device_id !== blob.device_id) throw new PwaError('wrong_user');
    const keys = new Map<number, Uint8Array>();
    for (const [e, hex] of Object.entries(secret.epochs)) keys.set(Number(e), fromHex(hex));
    if (!keys.has(secret.current_epoch)) throw new PwaError('shape');
    return new PwaSession(aead, secret.account_id, secret.device_id, keys, secret.current_epoch);
  }

  get locked() {
    return this.keys === null;
  }
  private key(epoch: number) {
    if (!this.keys) throw new PwaError('locked');
    const k = this.keys.get(epoch);
    if (!k) throw new PwaError('unknown_epoch');
    return k;
  }
  private checkOwner(env: Envelope) {
    if (env.account_id !== this.accountId) throw new PwaError('wrong_user');
  }

  private openEnvelope<T>(env: Envelope, kind: string): T {
    this.checkOwner(env);
    const key = this.key(env.epoch);
    try {
      const pt = this.aead.open(key, fromHex(env.nonce), aad(kind, env.account_id, env.device_id, env.epoch, env.op_id), fromHex(env.ciphertext));
      return JSON.parse(dec.decode(pt)) as T;
    } catch (e) {
      if (e instanceof PwaError) throw e;
      throw new PwaError('auth');
    }
  }

  /** Install a sealed dataset snapshot (selected agenda/tasks/briefing). Replaces prior data. */
  loadSnapshot(env: Envelope, nowMs: number): void {
    const snap = this.openEnvelope<Snapshot>(env, 'snapshot');
    if (!Array.isArray(snap.tasks) || !Array.isArray(snap.events)) throw new PwaError('shape');
    this.data = snap;
    this.syncedAt = nowMs;
    this.conflicts = [];
  }

  view(): Snapshot | null {
    if (!this.keys) throw new PwaError('locked');
    return this.data;
  }

  freshness(nowMs: number, staleAfterMs = 15 * 60_000): Freshness {
    if (this.syncedAt === null) return { state: 'never' };
    const ageMs = nowMs - this.syncedAt;
    return { state: ageMs > staleAfterMs ? 'stale' : 'fresh', ageMs };
  }

  private enqueue(entityId: string, base: number, row: TaskRow): void {
    const opId = crypto.randomUUID();
    const nonce = crypto.getRandomValues(new Uint8Array(12));
    const payload = enc.encode(JSON.stringify({ entity_type: 'task', entity_id: entityId, base_revision: base, logical_seq: ++this.seq, row }));
    const sealed = this.aead.seal(this.key(this.epoch), nonce, aad('op', this.accountId, this.deviceId, this.epoch, opId), payload);
    this.outbox.push({ op_id: opId, account_id: this.accountId, device_id: this.deviceId, epoch: this.epoch, nonce: toHex(nonce), ciphertext: toHex(sealed) });
  }

  /** Quick add works offline: it commits to the local view and the outbox together. */
  quickAdd(title: string, nowIso: string): TaskRow {
    if (!this.keys || !this.data) throw new PwaError('locked');
    const t = title.trim();
    if (!t || t.length > 500) throw new PwaError('shape');
    const row: TaskRow = {
      id: crypto.randomUUID(),
      revision: 1,
      created_at: nowIso,
      updated_at: nowIso,
      origin_device_id: this.deviceId,
      deleted_at: null,
      title: t,
      status: 'todo',
    };
    this.enqueue(row.id, 0, row);
    this.data.tasks.push(row);
    return row;
  }

  complete(taskId: string, nowIso: string): TaskRow {
    if (!this.keys || !this.data) throw new PwaError('locked');
    const cur = this.data.tasks.find((t) => t.id === taskId);
    if (!cur) throw new PwaError('shape');
    const next: TaskRow = { ...cur, status: 'done', revision: cur.revision + 1, updated_at: nowIso, origin_device_id: this.deviceId };
    this.enqueue(taskId, cur.revision, next);
    this.pendingBase.set(taskId, { base: cur.revision, status: 'done' });
    Object.assign(cur, next);
    return cur;
  }

  pendingOps(): readonly Envelope[] {
    return this.outbox;
  }

  /** Relay acknowledged these op ids; only then do they leave the outbox. */
  acknowledge(opIds: string[]): void {
    this.outbox = this.outbox.filter((e) => !opIds.includes(e.op_id));
  }

  /**
   * Apply an authenticated remote op for a task. A remote revision newer than the base of our own
   * unacknowledged edit is a visible conflict, never silent last-write-wins.
   */
  applyRemote(env: Envelope): void {
    if (!this.data) throw new PwaError('locked');
    const p = this.openEnvelope<{ entity_type: string; entity_id: string; row: TaskRow }>(env, 'op');
    if (p.entity_type !== 'task' || p.row?.id !== p.entity_id) throw new PwaError('shape');
    const local = this.data.tasks.find((t) => t.id === p.entity_id);
    const pending = this.pendingBase.get(p.entity_id);
    if (local && pending && p.row.revision > pending.base) {
      this.conflicts.push({ entity_id: p.entity_id, local_status: pending.status, remote_revision: p.row.revision, base_revision: pending.base });
      return; // local edit stays queued; the remote version is surfaced for the user
    }
    if (local) Object.assign(local, p.row);
    else this.data.tasks.push(p.row);
  }

  /** Lock refuses to discard unsent edits unless forced. */
  lock(force = false): void {
    if (!force && this.outbox.length > 0) throw new PwaError('pending');
    this.wipe();
  }

  /** Logout always clears key material, decrypted data and queued edits. */
  logout(): void {
    this.wipe();
  }

  private wipe() {
    this.keys?.forEach((k) => k.fill(0));
    this.keys = null;
    this.data = null;
    this.syncedAt = null;
    this.outbox = [];
    this.pendingBase.clear();
    this.conflicts = [];
  }
}

/** Desktop-side helper (desktop snapshot producer is not built yet): seal a snapshot under an epoch key. */
export function sealSnapshot(aead: Aead, key: Uint8Array, account: string, device: string, epoch: number, id: string, snap: Snapshot): Envelope {
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const ct = aead.seal(key, nonce, aad('snapshot', account, device, epoch, id), enc.encode(JSON.stringify(snap)));
  return { op_id: id, account_id: account, device_id: device, epoch, nonce: toHex(nonce), ciphertext: toHex(ct) };
}
