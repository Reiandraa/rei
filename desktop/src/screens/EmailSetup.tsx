import { useRef, useState, type FormEvent } from 'react';
import { api, asIpcError, type EmailProfile, type ProviderAccount } from '../ipc';

/** One entry point. Native code discovers the provider and owns OAuth/tokens. */
export function EmailSetup({onHiz, account, onConnected, storageReady = true}: {onHiz: (suggestedId: string) => void; account?: ProviderAccount; onConnected?: () => Promise<void> | void; storageReady?: boolean}) {
  const [email, setEmail] = useState(String(account?.config.email ?? ''));
  const [password, setPassword] = useState('');
  const [profile, setProfile] = useState<EmailProfile | null>(null);
  const [username, setUsername] = useState(String(account?.config.username ?? ''));
  const [busy, setBusy] = useState(false);
  const [phase, setPhase] = useState('');
  const [error, setError] = useState('');
  const pendingId = useRef<string | null>(null);
  const running = useRef(false);
  const isCloud = account?.provider === 'google_mail' || account?.provider === 'microsoft_mail';
  const isHiz = account?.provider === 'hiz_imap';
  async function connect(preferred?: 'google' | 'microsoft') {
    if (running.current || !storageReady || account?.fetch_running) return;
    running.current = true; setBusy(true); setError('');
    const entered = password; setPassword('');
    try {
      let id = preferred ? undefined : account?.account_id ?? pendingId.current;
      let cloud = isCloud || profile?.kind === 'google' || profile?.kind === 'microsoft';
      if (!id) {
        setPhase('Finding your email connection…');
        const found = (!preferred && profile) || await api.emailDiscover(email.trim(), preferred);
        setProfile(found);
        cloud = found.kind === 'google' || found.kind === 'microsoft';
        if (cloud && !found.publisher_ready) throw new Error(`${found.display_name} sign-in is not enabled in this beta yet. The REI publisher must finish provider setup; you do not need developer settings.`);
        const login = username || found.username;
        setUsername(login);
        if (!cloud && (!entered || !login)) {
          setError(!login ? 'This email service uses a separate sign-in ID. Enter the ID you use for its webmail below.' : 'Enter your email password, then choose Connect email.');
          return;
        }
        id = await api.emailConfigure(found.profile_id, cloud ? undefined : login);
        pendingId.current = id;
      }
      setPhase(cloud ? 'Finish sign-in in your browser, then return to REI…' : 'Connecting securely to your email…');
      if (isHiz) await api.hizSignIn(id, entered);
      else await api.emailSignIn(id, cloud ? undefined : entered);
      setPhase('Starting your automatic inbox…');
      await api.assistantWorkflowSet(true, id);
      window.dispatchEvent(new Event('rei-assistant-changed'));
      await onConnected?.();
      setPhase('Email connected. REI will check new messages automatically.');
    } catch(e) {
      if (!pendingId.current && asIpcError(e).code === 'Validation') setProfile(null);
      setError(e instanceof Error ? e.message : asIpcError(e).message);
    }
    finally { running.current = false; setBusy(false); }
  }
  function submit(e: FormEvent) { e.preventDefault(); void connect(); }
  function editEmail(value: string) { setEmail(value); setProfile(null); setUsername(''); pendingId.current = null; setPassword(''); setError(''); }
  function provider(preferred: 'google' | 'microsoft') {
    setProfile(null); pendingId.current = null;
    void connect(preferred);
  }
  const blocked = busy || !storageReady || Boolean(account?.fetch_running);
  const needsId = isHiz || profile?.kind === 'hiz' || profile?.kind === 'imap' && profile.username !== profile.email;
  return <div className="email-setup">
    <h3>Connect email</h3><p className="muted">Sign in once. REI reads your Inbox without marking messages read or sending email.</p>
    <form onSubmit={submit} aria-label="Connect email">
      <fieldset disabled={blocked}>
        {!isHiz && <label className="field">Email<input type="email" autoComplete="email" value={email} disabled={Boolean(account)} onChange={e=>editEmail(e.target.value)} placeholder="you@example.com" required maxLength={254}/></label>}
        {needsId && <label className="field">Sign-in ID<input autoComplete="username" value={username} disabled={Boolean(account?.sign_in_saved)} onChange={e=>setUsername(e.target.value)} maxLength={254}/></label>}
        {!isCloud && profile?.kind !== 'google' && profile?.kind !== 'microsoft' && <label className="field">Password<input type="password" autoComplete="off" value={password} onChange={e=>setPassword(e.target.value)} maxLength={1024}/></label>}
        {profile && <p className="email-detected">{profile.display_name}{profile.server ? ` · ${profile.server} · encrypted connection` : ' · official browser sign-in'}</p>}
        <button className="primary" type="submit">{busy ? 'Connecting…' : isCloud ? 'Sign in again in browser' : 'Connect email'}</button>
      </fieldset>
    </form>
    {!account && <><div className="email-login-divider"><span>or</span></div><div className="email-browser-options"><button disabled={blocked} onClick={()=>provider('google')}>Continue with Google</button><button disabled={blocked} onClick={()=>provider('microsoft')}>Continue with Microsoft</button></div></>}
    <p className="muted email-login-help">Google or Microsoft account? Use its sign-in button. Your password stays on the provider’s official page.</p>
    <details className="email-connection-help"><summary>How your connection works</summary><p className="muted">For other addresses REI finds encrypted email settings automatically. Only the domain is shared with Thunderbird’s provider directory, never your password. Some providers require an app password or restrict outside apps.</p><p className="muted">Sign-in is saved in this computer’s secure storage. REI checks your Inbox while the app is open and the computer is awake.</p></details>
    {account?.fetch_running && !busy && <p role="status">An email operation is already running. <button onClick={()=>void api.fetchCancel(account.account_id).catch(e=>setError(asIpcError(e).message))}>Cancel connection</button></p>}
    {busy && <div role="status"><p>{phase}</p>{pendingId.current && <button onClick={()=>void api.fetchCancel(pendingId.current!).catch(e=>setError(asIpcError(e).message))}>Cancel connection</button>}</div>}
    {!busy && phase.startsWith('Email connected') && <p role="status" className="success">{phase}</p>}
    {error && <p role="alert">{error}</p>}
    {!account && <details><summary>My university uses a HIZ sign-in ID</summary><p className="muted">Saarland University HIZ email can use a separate campus ID. Server settings are supplied by REI.</p><button type="button" onClick={()=>{setPassword('');onHiz('');}}>I already have a HIZ sign-in ID</button></details>}
  </div>;
}
