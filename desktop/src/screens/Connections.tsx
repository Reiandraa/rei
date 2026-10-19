// Connections: HIZ mailbox (IMAP/TLS) and Microsoft Teams setup, source selection,
// Fetch now/cancel, disconnect and opt-in polling. Every control calls real Rust IPC.
// Status distinguishes saved settings, saved sign-in, successful read and last attempt.
import { useCallback, useEffect, useState } from 'react';
import { api, asIpcError, type ConnectionsStatus, type ProviderAccount } from '../ipc';

function when(iso: string | null) {
  return iso ? new Date(iso).toLocaleString('en-GB', { dateStyle: 'medium', timeStyle: 'short' }) : 'never';
}

const FAIL: Record<string, string> = { degraded: 'Problem fetching', expired: 'Sign-in expired', permission_denied: 'Access declined' };

function stateText(a: ProviderAccount) {
  if (a.evidence === 'fixture') return 'Synthetic test data only — not a connected account';
  if (a.state === 'connected') return 'Connected (a real read succeeded)';
  if (a.state === 'not_configured') return 'Disconnected';
  if (FAIL[a.state]) return FAIL[a.state];
  if (!a.sign_in_saved) return 'Settings saved — sign-in needed';
  return a.last_read_ok_at ? a.state : 'Signed in — no successful read yet';
}

type Run = (f: () => Promise<unknown>, ok: string) => Promise<void>;

function AccountCard({ a, run }: { a: ProviderAccount; run: Run }) {
  const [pw, setPw] = useState('');
  const [sel, setSel] = useState<Set<string>>(new Set(a.sources.filter((s) => s.selected).map((s) => `${s.source_kind}\n${s.source_ref}`)));
  const [confirmPurge, setConfirmPurge] = useState(false);
  const hiz = a.provider === 'hiz_imap';
  const teamsIdentity = a.provider === 'teams_graph' && a.kind === 'teams_chat';
  const toggle = (k: string) => { const n = new Set(sel); if (n.has(k)) n.delete(k); else n.add(k); setSel(n); };
  return (
    <article className="card" aria-label={`Connection: ${a.display_name}`}>
      <h3>{a.display_name}</h3>
      <p>Status: <strong>{stateText(a)}</strong></p>
      <p className="muted">Sign-in saved: {a.sign_in_saved ? 'yes (OS credential store)' : 'no'} · last successful read {when(a.last_read_ok_at)} ·
        last attempt {when(a.last_attempt_at)} · {a.stored_messages} stored{a.last_outcome ? ` · ${a.last_outcome}` : ''}</p>
      {a.last_error && <p role="alert">{a.last_error}</p>}
      {hiz && (
        <div>
          <label>HIZ password <input type="password" autoComplete="off" value={pw} onChange={(e) => setPw(e.target.value)} /></label>
          <button disabled={!pw} onClick={() => { const p = pw; setPw(''); void run(() => api.hizSignIn(a.account_id, p), 'Signed in to HIZ; folders listed.'); }}>
            Sign in and list folders</button>
        </div>
      )}
      {teamsIdentity && (
        <div>
          <button onClick={() => void run(() => api.teamsSignIn(a.account_id), 'Signed in with Microsoft.')}>Sign in with Microsoft (opens your browser)</button>
          <button disabled={!a.sign_in_saved} onClick={() => void run(() => api.teamsDiscover(a.account_id), 'Chats and channels listed.')}>List my chats and channels</button>
        </div>
      )}
      {a.sources.length > 0 && (
        <fieldset>
          <legend>Choose what to read ({hiz ? 'folders' : a.kind === 'teams_chat' ? 'chats' : 'channels, including replies'})</legend>
          {a.sources.map((s) => {
            const k = `${s.source_kind}\n${s.source_ref}`;
            return <label key={k}><input type="checkbox" checked={sel.has(k)} onChange={() => toggle(k)} /> {s.display_name}</label>;
          })}
          <button onClick={() => void run(() => api.selectSources(a.account_id, [...sel].map((k) => {
            const [source_kind, source_ref] = k.split('\n'); return { source_kind, source_ref };
          })), 'Selection saved.')}>Save selection</button>
        </fieldset>
      )}
      <div className="actions">
        <button disabled={!a.sign_in_saved || a.fetch_running || !a.sources.some((s) => s.selected)}
          onClick={() => void run(() => api.fetchNow(a.account_id), 'Fetch finished.')}>Fetch now</button>
        <button disabled={!a.fetch_running} onClick={() => void run(() => api.fetchCancel(a.account_id), 'Cancel requested.')}>Cancel fetch</button>
        {!confirmPurge ? <button onClick={() => setConfirmPurge(true)}>Disconnect…</button> : (
          <span role="group" aria-label="Confirm disconnect">
            Removes the saved sign-in. Keep the stored messages?{' '}
            <button onClick={() => void run(() => api.disconnect(a.account_id, false), 'Disconnected; messages kept.')}>Disconnect, keep messages</button>
            <button onClick={() => void run(() => api.disconnect(a.account_id, true), 'Disconnected; messages deleted.')}>Disconnect and delete messages</button>
            <button onClick={() => setConfirmPurge(false)}>Keep connection</button>
          </span>
        )}
      </div>
    </article>
  );
}

function MailboxPreferences({a,run}:{a:ProviderAccount;run:Run}) {
  const [password,setPassword]=useState('');
  const [selected,setSelected]=useState(()=>new Set(a.sources.filter(s=>s.selected).map(s=>s.source_ref)));
  const [busy,setBusy]=useState(false);
  const [disconnecting,setDisconnecting]=useState(false);
  async function act(f:()=>Promise<unknown>,message:string){if(busy)return;setBusy(true);try{await run(f,message);}finally{setBusy(false);}}
  return <section className="mailbox-preferences" aria-label="Mailbox preferences">
    <div className="mailbox-toolbar">
    <p className="mailbox-status"><strong>{a.state==='connected' && a.evidence!=='fixture'?'Email connected':stateText(a)}</strong><span>{a.stored_messages} messages stored · Last checked {when(a.last_read_ok_at)}</span></p>
    <div className="actions"><button disabled={busy || !a.sign_in_saved || a.fetch_running || !a.sources.some(s=>s.selected)} onClick={()=>void act(()=>api.fetchNow(a.account_id),'Email check finished.')}>Check email now</button>{a.fetch_running && <button onClick={()=>void run(()=>api.fetchCancel(a.account_id),'Cancellation requested.')}>Cancel check</button>}</div>
    </div>
    {a.last_error && <p role="alert">{a.last_error}</p>}
    <fieldset disabled={busy} className="folder-choices"><legend>Mail folders to read</legend><p className="muted">Inbox is enough for most people. REI leaves messages unread on the server.</p>{a.sources.map(source=><label key={source.source_ref}><input type="checkbox" checked={selected.has(source.source_ref)} onChange={()=>{const next=new Set(selected);if(next.has(source.source_ref))next.delete(source.source_ref);else next.add(source.source_ref);setSelected(next);}}/>{source.display_name}</label>)}
    {a.sources.length>0?<button onClick={()=>void act(()=>api.selectSources(a.account_id,[...selected].map(source_ref=>({source_kind:'folder',source_ref}))),'Mail folders saved.')}>Save folders</button>:<p>Reconnect below to list your mail folders.</p>}</fieldset>
    <details className="settings-subsection"><summary>Reconnect email</summary><p className="muted">Only needed when your password changes or sign-in expires. Your stored messages are kept.</p><label className="field">HIZ password<input type="password" autoComplete="off" value={password} onChange={e=>setPassword(e.target.value)}/></label><button disabled={busy || !password} onClick={()=>{const pw=password;setPassword('');void act(()=>api.hizSignIn(a.account_id,pw),'Email reconnected; folders refreshed.');}}>Reconnect and list folders</button></details>
    <details className="settings-subsection"><summary>Connection information</summary><p className="muted">Sign-in saved: {a.sign_in_saved?'yes, in this computer’s secure storage':'no'}. Last attempt: {when(a.last_attempt_at)}.</p>{a.last_outcome && <p className="muted">{a.last_outcome}</p>}</details>
    <details className="settings-subsection disconnect-settings"><summary>Disconnect email</summary><p className="muted">Removes the saved sign-in on this computer. Choose whether to keep messages already stored in REI.</p>{!disconnecting?<button disabled={busy} onClick={()=>setDisconnecting(true)}>Disconnect…</button>:<div role="group" aria-label="Confirm disconnect"><button disabled={busy} onClick={()=>void act(()=>api.disconnect(a.account_id,false),'Disconnected; messages kept.')}>Disconnect, keep messages</button><button className="danger" disabled={busy} onClick={()=>void act(()=>api.disconnect(a.account_id,true),'Disconnected; messages deleted.')}>Disconnect and delete messages</button><button onClick={()=>setDisconnecting(false)}>Keep connection</button></div>}</details>
  </section>;
}

export function ConnectionsPanel({ onChanged, showTeams = true, compact = false }: { onChanged?: () => Promise<void> | void; showTeams?: boolean; compact?: boolean }) {
  const [st, setSt] = useState<ConnectionsStatus | null>(null);
  const [msg, setMsg] = useState<string | null>(null);
  const [active, setActive] = useState(0);
  const [hiz, setHiz] = useState({ username: '', initial: 50 });
  const [tm, setTm] = useState({ client: '', tenant: 'organizations', channels: false });
  const [poll, setPoll] = useState({ enabled: false, minutes: 30 });
  const load = useCallback(async () => {
    try { const s = await api.connectionsStatus(); setSt(s); setPoll({ enabled: s.polling.enabled, minutes: s.polling.interval_minutes }); }
    catch (e) { setMsg(asIpcError(e).message); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  // Sign-in/fetch runs asynchronously in Rust. Refresh its running state while
  // awaiting IPC so Cancel can become usable before the original call returns.
  useEffect(() => {
    if (!active) return;
    const timer = window.setInterval(() => { void load(); }, 1000);
    return () => window.clearInterval(timer);
  }, [active, load]);
  const run: Run = async (f, ok) => {
    setActive((n) => n + 1);
    try { await f(); setMsg(ok); } catch (e) { setMsg(asIpcError(e).message); }
    finally { setActive((n) => n - 1); await load(); await onChanged?.(); }
  };
  if (!st) return msg ? <section className="card" aria-label="Connections setup"><h2>Connections</h2><p role="alert">{msg}</p></section> : null;
  const hizAcct = st.accounts.find((a) => a.provider === 'hiz_imap');
  const teamsAcct = st.accounts.find((a) => a.provider === 'teams_graph' && a.kind === 'teams_chat');
  if(compact)return <section className="connection-preferences" aria-label="Email preferences controls">
    {msg && <p role="status">{msg}</p>}{!st.os_vault_available && <p role="alert">Secure storage is unavailable; sign-in cannot be saved.</p>}
    {hizAcct?<MailboxPreferences key={`${hizAcct.account_id}:${JSON.stringify(hizAcct.sources)}:${hizAcct.state}`} a={hizAcct} run={run}/>:<p>Connect your university email in Automatic inbox above.</p>}
    <fieldset className="email-interval"><legend>How often to check</legend><p className="muted">Only while REI is open and your computer is awake. Pause or Resume in Automatic inbox controls both email checks and AI reading.</p><label className="field">Minutes between checks<input type="number" min={15} max={1440} value={poll.minutes} onChange={e=>setPoll({...poll,minutes:Number(e.target.value)})}/></label><button disabled={active>0 || !Number.isInteger(poll.minutes) || poll.minutes<15 || poll.minutes>1440} onClick={()=>void run(async()=>{const fresh=await api.connectionsStatus();return api.pollingSet(fresh.polling.enabled,poll.minutes);},'Check interval saved.')}>Save interval</button></fieldset>
    {hizAcct && <details className="settings-subsection"><summary>Account details</summary><p className="muted">Server {st.hiz_host}:{st.hiz_port}, IMAP over TLS. Change these account details only to correct setup.</p><label className="field">HIZ identifier<input defaultValue={String(hizAcct.config.username??'')} onChange={e=>setHiz({...hiz,username:e.target.value})}/></label><label className="field">Messages per folder on first import<input type="number" min={1} max={500} value={hiz.initial} onChange={e=>setHiz({...hiz,initial:Number(e.target.value)})}/></label><button disabled={active>0 || !hiz.username.trim()} onClick={()=>void run(()=>api.hizConfigure(hizAcct.account_id,hiz.username,hiz.initial),'Account details saved.')}>Save account details</button></details>}
  </section>;

  return (
    <section className="card" aria-label="Connections setup">
      <h2>Connections</h2>
      {msg && <p role="status">{msg}</p>}
      {!st.os_vault_available && <p role="alert">The OS credential store is unavailable on this system, so no sign-in can be saved.</p>}
      <details open={!hizAcct}>
        <summary>HIZ email</summary>
        <p className="muted">Server: {st.hiz_host}:{st.hiz_port}, IMAP over TLS — the documented HIZ setting, not yet confirmed with your account.
          Read-only: messages stay unread on the server. Use your HIZ identifier, not your Microsoft login. Being signed in to HIZ webmail in a browser does not sign this app in;
          REI asks for the identifier and password once and keeps the password in this computer’s secure storage.</p>
        <label>HIZ identifier <input value={hiz.username} onChange={(e) => setHiz({ ...hiz, username: e.target.value })} /></label>
        <label>Recent messages per folder on first fetch <input type="number" min={1} max={500} value={hiz.initial}
          onChange={(e) => setHiz({ ...hiz, initial: Number(e.target.value) })} /></label>
        <button disabled={!hiz.username.trim()} onClick={() => void run(() => api.hizConfigure(hizAcct?.account_id ?? null, hiz.username, hiz.initial), 'HIZ settings saved.')}>
          Save HIZ settings</button>
      </details>
      {showTeams && <details open={!teamsAcct}>
        <summary>Microsoft Teams (work/school account)</summary>
        {!teamsAcct && <p>Setup pending: Teams cannot be connected until an app registration exists for your university.
          Your email address or password cannot replace the application ID, and there is no one-click sign-in yet. HIZ email works without this.</p>}
        <details><summary>Advanced connection settings</summary>
        <p className="muted">Needs an application (client) ID from an app registration in your university tenant (public client, redirect
          http://127.0.0.1:43821/callback as a public-client redirect; the exact port and path must match).
          A 127.0.0.1 HTTP redirect may need the app manifest rather than the portal text field.
          Channel reading may need tenant admin consent.</p>
        <label>Application (client) ID <input value={tm.client} onChange={(e) => setTm({ ...tm, client: e.target.value })} /></label>
        <label>Tenant (“organizations” or tenant ID) <input value={tm.tenant} onChange={(e) => setTm({ ...tm, tenant: e.target.value })} /></label>
        <label><input type="checkbox" checked={tm.channels} onChange={(e) => setTm({ ...tm, channels: e.target.checked })} /> Also read selected team channels</label>
        <button disabled={!tm.client.trim()} onClick={() => void run(() => api.teamsConfigure(teamsAcct?.account_id ?? null, tm.client, tm.tenant, tm.channels), 'Teams settings saved.')}>
          Save Teams settings</button>
        </details>
      </details>}
      {st.accounts.filter(a=>showTeams || a.provider!=='teams_graph').map((a) => <AccountCard key={`${a.account_id}:${a.sources.length}:${a.state}`} a={a} run={run} />)}
      <fieldset>
        <legend>Automatic fetching (optional)</legend>
        <p className="muted">Only while REI OS is open and the computer is awake. One account at a time; waits longer after errors.</p>
        <label><input type="checkbox" checked={poll.enabled} onChange={(e) => setPoll({ ...poll, enabled: e.target.checked })} /> Fetch automatically</label>
        <label>Every (minutes, 15–1440) <input type="number" min={15} max={1440} value={poll.minutes}
          onChange={(e) => setPoll({ ...poll, minutes: Number(e.target.value) })} /></label>
        <button onClick={() => void run(() => api.pollingSet(poll.enabled, poll.minutes), poll.enabled ? 'Automatic fetching on.' : 'Automatic fetching off.')}>Save schedule</button>
      </fieldset>
    </section>
  );
}
