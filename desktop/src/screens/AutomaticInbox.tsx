// One ordinary setup flow. Credentials only go through the existing password IPC.
// Stored status is polled; no provider request runs merely because Settings is opened.
import { useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { api, asIpcError, isApiModel, type AutoSettings, type ConnectionsStatus } from '../ipc';
import { EmailSetup } from './EmailSetup';
import { DisconnectEmail } from '../components/DisconnectEmail';

const when = (s: string | null) => s ? new Date(s).toLocaleString('en-GB', { dateStyle: 'medium', timeStyle: 'short' }) : 'not checked yet';
export function AutomaticInbox({ onAdvanced }: { onAdvanced?: () => void }) {
  const [data, setData] = useState<{ sources: ConnectionsStatus; assistant: AutoSettings } | null>(null);
  const [username, setUsername] = useState('');
  const [providerChosen, setProviderChosen] = useState(false);
  const [password, setPassword] = useState('');
  const [setupId, setSetupId] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const version = useRef(0);
  const load = useCallback(async () => {
    const seq = ++version.current;
    try {
      const [sources, assistant] = await Promise.all([api.connectionsStatus(), api.autoSettingsGet()]);
      if (seq === version.current) { setData({ sources, assistant }); setLoadError(null); }
    } catch (e) { if (seq === version.current) setLoadError(asIpcError(e).message); }
  }, []);
  const invalidate = useCallback(() => { version.current++; }, []);
  useEffect(() => {
    void load(); const t = setInterval(() => void load(),10000);
    const refresh = () => void load(); window.addEventListener('rei-assistant-changed',refresh);
    return () => { invalidate(); clearInterval(t); window.removeEventListener('rei-assistant-changed',refresh); };
  }, [load, invalidate]);
  const account = data?.sources.accounts.find(a => a.kind === 'email' && a.sign_in_saved && a.state !== 'not_configured') ?? data?.sources.accounts.find(a => a.kind === 'email' && a.state !== 'not_configured');
  const savedName = typeof account?.config.username === 'string' ? account.config.username : '';
  useEffect(() => { if (savedName) setUsername(savedName); }, [savedName]);
  const needsLogin = !account?.sign_in_saved || ['not_configured','expired','permission_denied'].includes(account.state);
  const needsScope = Boolean(account && !account.sources.some(s => s.source_kind === 'folder' && s.selected));
  const on = Boolean(data?.assistant.enabled && !data.assistant.paused && data.sources.polling.enabled && !needsLogin && !needsScope);
  async function connect(e: FormEvent) {
    e.preventDefault(); if (busy) return;
    const transient = password; setPassword(''); setBusy(true); setError(null); setNotice(null);
    try {
      let id = account?.account_id ?? setupId;
      if (!id) { id = await api.hizConfigure(null,username.trim(),50); setSetupId(id); }
      else if (!account?.sign_in_saved && username.trim() !== savedName) { await api.hizConfigure(id,username.trim(),50); }
      await api.hizSignIn(id,transient);
      await api.assistantWorkflowSet(true,id);
      setNotice('Sign-in saved. REI is checking your email and will sort new messages automatically.');
      window.dispatchEvent(new Event('rei-assistant-changed'));
    } catch (e) { setError(asIpcError(e).message); }
    finally { await load(); setBusy(false); }
  }
  async function control(enabled: boolean, retry = false) {
    if (busy) return; setBusy(true); setError(null); setNotice(null);
    try {
      await api.assistantWorkflowSet(enabled,enabled ? account?.account_id ?? null : null);
      if (retry) await api.autoQueueRetry(null);
      setNotice(enabled ? 'Automatic inbox is on. REI checks now and keeps working in the background.' : 'Paused. Your sign-in, messages and tasks are kept.');
      window.dispatchEvent(new Event('rei-assistant-changed'));
    } catch(e) { setError(asIpcError(e).message); }
    finally { await load(); setBusy(false); }
  }
  async function disconnected() {
    setSetupId(null);setProviderChosen(false);setPassword('');setUsername('');setError(null);
    setNotice('Email disconnected. Your messages and tasks are kept.');
    window.dispatchEvent(new Event('rei-assistant-changed'));await load();
  }
  const problem = data?.assistant.worker_state;
  return <section className="card automatic-inbox" aria-label="Automatic inbox">
    <span className="overline">Your university, in one place</span><h2>Automatic inbox</h2>
    <p className="muted">Connect once. REI checks your email, separates important updates from routine messages, and prepares tasks for your daily review.</p>
    {!data ? <p role="status">Loading your connection…</p> : <>
      {!data.sources.os_vault_available && <p role="alert">Secure sign-in storage is unavailable. Nothing can be saved until it is working.</p>}
      {needsLogin && !providerChosen ? <EmailSetup storageReady={data.sources.os_vault_available} account={account} onConnected={load} onHiz={id=>{setUsername(id);setPassword('');setError(null);setProviderChosen(true);}}/> : needsLogin ? <form onSubmit={e => void connect(e)} aria-label="Connect university email"><fieldset disabled={busy || !data.sources.os_vault_available}>
        <div className="email-provider-heading"><strong>Saarland University · HIZ email</strong>{!account && <button type="button" onClick={()=>{setPassword('');setUsername('');setError(null);setProviderChosen(false);}}>Change email</button>}</div>
        <label className="field">University ID<input autoComplete="username" value={username} disabled={Boolean(account?.sign_in_saved)} onChange={e=>setUsername(e.target.value)} required maxLength={254} /></label>
        <p className="muted">Use your HIZ webmail sign-in ID and password. The ID can differ from your email address or Microsoft login. Server settings are filled in for you. Your password stays in this computer’s secure storage.</p>
        <label className="field">Password<input type="password" autoComplete="off" value={password} onChange={e=>setPassword(e.target.value)} required /></label>
        <p className="muted">New connections start with the latest 50 Inbox messages. Existing folder choices are kept. Nothing is marked read or deleted.</p>
        <button className="primary" type="submit" disabled={!username.trim() || !password}>{busy ? 'Connecting…' : account ? 'Sign in and resume' : 'Connect and start'}</button>
      </fieldset></form> : <>
        <div className="connection-summary"><span className="connection-dot" /><div><strong>Email</strong><small>{savedName || 'Saved sign-in'} · {account.evidence === 'fixture' ? 'Synthetic test connection only' : 'Sign-in saved securely'}</small></div><span className="connection-state">{on ? 'Automatic' : 'Paused'}</span><DisconnectEmail key={account.account_id} accountId={account.account_id} disabled={busy || account.fetch_running} onDisconnected={disconnected}/></div>
        <p className="muted">{account.fetch_running ? 'Checking for new messages…' : `Last successful check: ${when(account.last_read_ok_at)}`} · {account.stored_messages} stored</p>
        {account.last_error && <p role="alert">{account.last_error} {on && ['degraded'].includes(account.state) && 'Your sign-in is kept; REI will retry automatically.'}</p>}
        {on && problem === 'offline' && <p role="status">Email stays connected. {isApiModel(data.assistant.model)?<>{data.assistant.worker_detail} <a href="#/settings?tab=ai">Open AI settings →</a></>:'Open Ollama on this computer; REI retries local reading automatically.'}</p>}
        {on && problem === 'model_missing' && <p role="status">Email stays connected. Choose an installed model to start local reading. <button onClick={onAdvanced}>Choose local model</button></p>}
        {on && problem === 'error' && <p role="status">Some messages could not be read. They are kept, and appear under Needs attention on Today.</p>}
        <div className="actions"><button disabled={busy} className={on ? undefined : 'primary'} onClick={()=>void control(!on)}>{busy ? 'Saving…' : on ? 'Pause automatic inbox' : data.assistant.enabled && !needsScope ? 'Resume automatic inbox' : 'Start automatic inbox'}</button>
          {on && ['offline','model_missing','error'].includes(problem ?? '') && <button disabled={busy} onClick={()=>void control(true,true)}>Try again</button>}
          <a className="button" href="#/today">Open daily review →</a></div>
      </>}
      {needsLogin && account?.sign_in_saved && <DisconnectEmail key={account.account_id} accountId={account.account_id} disabled={busy || account.fetch_running} onDisconnected={disconnected}/>}
      <p className="connection-help">Checks your connected, selected sources every {data.sources.polling.interval_minutes} minutes while REI is open and this computer is awake. Your selected AI reads new and changed messages automatically. Tasks wait for your approval; routine messages stay available in Messages.</p>

    </>}
    {notice && <p role="status" className="success">{notice}</p>}{error && <p role="alert">{error}</p>}{loadError && <p role="alert">{loadError} <button onClick={()=>void load()}>Retry connection status</button></p>}
  </section>;
}
