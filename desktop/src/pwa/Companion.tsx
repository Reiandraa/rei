import { useState } from 'react';
import { PwaError, PwaSession, type Aead, type Envelope } from './core';

interface Props {
  aead: Aead | null;
  /** Encrypted pairing offer delivered by the relay; null until the user fetches it. */
  offer: Envelope | null;
  snapshot: Envelope | null;
  now?: () => number;
}

const MSG: Record<string, string> = {
  auth: 'Pairing code is wrong or the data was tampered with.',
  wrong_user: 'This data belongs to a different account.',
  no_aead: 'Encryption library is not installed in this build; the companion cannot unlock.',
  locked: 'Locked.',
  pending: 'Unsent edits exist. Sync first or choose Discard and lock.',
  shape: 'Invalid input.',
  unknown_epoch: 'Key epoch unknown; re-pair this device.',
};

/** Limited iPhone companion: agenda, tasks, quick add/complete, freshness. No inbox, no AI. */
export function Companion({ aead, offer, snapshot, now = Date.now }: Props) {
  const [session, setSession] = useState<PwaSession | null>(null);
  const [code, setCode] = useState('');
  const [title, setTitle] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [, bump] = useState(0);
  const refresh = () => bump((n) => n + 1);
  const guard = (fn: () => void) => {
    try {
      setError(null);
      fn();
      refresh();
    } catch (e) {
      setError(e instanceof PwaError ? (MSG[e.code] ?? e.code) : 'Unexpected error');
    }
  };

  if (!session || session.locked) {
    return (
      <main aria-label="REI companion">
        <h1>REI companion</h1>
        <p>Enter the pairing code shown on your desktop.</p>
        <input aria-label="Pairing code" value={code} onChange={(e) => setCode(e.target.value)} autoComplete="off" />
        <button
          disabled={!offer}
          onClick={() =>
            void (async () => {
              try {
                setError(null);
                if (!offer) return;
                const s = await PwaSession.pair(code, offer, aead);
                setCode('');
                if (snapshot) s.loadSnapshot(snapshot, now());
                setSession(s);
              } catch (e) {
                setError(e instanceof PwaError ? (MSG[e.code] ?? e.code) : 'Unexpected error');
              }
            })()
          }
        >
          Unlock
        </button>
        {!offer && <p role="status">No pairing offer available. Pair from the desktop app first.</p>}
        {error && <p role="alert">{error}</p>}
      </main>
    );
  }

  const data = session.view();
  const fresh = session.freshness(now());
  return (
    <main aria-label="REI companion">
      <h1>Today</h1>
      <p role="status">
        {fresh.state === 'never'
          ? 'Never synced.'
          : fresh.state === 'stale'
            ? `Out of date: last synced ${Math.round(fresh.ageMs / 60000)} min ago.`
            : 'Up to date.'}
        {session.pendingOps().length > 0 && ` ${session.pendingOps().length} change(s) not yet sent.`}
      </p>
      {session.conflicts.length > 0 && <p role="alert">{session.conflicts.length} conflicting edit(s) need review on desktop.</p>}
      {data?.briefing && <p>{data.briefing}</p>}
      <h2>Agenda</h2>
      {data?.events.length ? <ul>{data.events.map((e) => <li key={e.id}>{e.title}</li>)}</ul> : <p>No events.</p>}
      <h2>Tasks</h2>
      {data?.tasks.length ? (
        <ul>
          {data.tasks.map((t) => (
            <li key={t.id}>
              {t.title} ({t.status}){' '}
              {t.status !== 'done' && <button onClick={() => guard(() => session.complete(t.id, new Date(now()).toISOString()))}>Complete {t.title}</button>}
            </li>
          ))}
        </ul>
      ) : (
        <p>No tasks.</p>
      )}
      <input aria-label="New task" value={title} onChange={(e) => setTitle(e.target.value)} />
      <button
        onClick={() =>
          guard(() => {
            session.quickAdd(title, new Date(now()).toISOString());
            setTitle('');
          })
        }
      >
        Add task
      </button>
      <button onClick={() => guard(() => session.lock())}>Lock</button>
      <button onClick={() => guard(() => session.lock(true))}>Discard and lock</button>
      <button onClick={() => guard(() => session.logout())}>Log out</button>
      {error && <p role="alert">{error}</p>}
    </main>
  );
}
