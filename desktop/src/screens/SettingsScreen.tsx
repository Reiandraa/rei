import { useCallback, useEffect, useState } from 'react';
import { api, asIpcError, isApiModel, type AppStatus, type Health, type DailySummary, type SettingKey } from '../ipc';
import { AiSettings } from './AiSettings';
import { s } from '../strings';
import { applyTheme, applyAccent, applyTextSize, applyMotion, resolveTheme, resolveAccent, ACCENTS } from '../theme';
import { AutomaticInbox } from './AutomaticInbox';
import { ConnectionsPanel } from './Connections';

/** Installed local models (fixed loopback Ollama); the choice drives the automatic worker. */
export function LocalAiSettings() {
  const [list, setList] = useState<{ state: string; models: string[] } | null>(null);
  const [cur, setCur] = useState<{ enabled: boolean; paused: boolean; model: string; date_zone?: string } | null>(null);
  const [manual, setManual] = useState('');
  const [busy, setBusy] = useState(false);
  const [zone, setZone] = useState('');
  const [msg, setMsg] = useState<string | null>(null);
  useEffect(() => {
    api.autoSettingsGet().then((a) => { setCur(a); setZone(a.date_zone ?? 'Europe/Berlin');if(!isApiModel(a.model))void api.modelList().then(setList,()=>setList({state:'error',models:[]})); }, (e) => setMsg(asIpcError(e).message));
  }, []);
  async function save(model: string | undefined, z?: string) {
    if (!cur || busy) return;
    setBusy(true);
    try {
      const fresh = await api.autoSettingsGet();
      const a = await api.autoSettingsSet(fresh.enabled, fresh.paused, model ?? fresh.model, z);
      setCur(a);
      window.dispatchEvent(new Event('rei-assistant-changed'));
      setMsg(`Saved. Automatic assistance now uses ${a.model}${a.date_zone ? ` and reads dates in ${a.date_zone}` : ''}.`);
    } catch (e) { setMsg(asIpcError(e).message); } finally { setBusy(false); }
  }
  return (
    <section className="card" aria-label="Local AI">
      <h2>{cur && isApiModel(cur.model)?'Date and time':'Local AI'}</h2>
      {!cur ? <p role="status">Loading…</p> : (
        <>
          {!isApiModel(cur.model) && <><p className="muted">Current model: <strong>{cur.model}</strong>. Only models already installed in Ollama on this computer are listed; nothing is downloaded.</p>
          {list?.state === 'ready' && list.models.length > 0 ? (
            <label className="field"><span>Installed model</span>
              <select disabled={busy} value={list.models.includes(cur.model) ? cur.model : ''} onChange={(e) => e.target.value && void save(e.target.value)}>
                {!list.models.includes(cur.model) && <option value="">— choose —</option>}
                {list.models.map((m) => <option key={m} value={m}>{m}</option>)}
              </select>
            </label>
          ) : (
            <p className="muted">{list === null ? 'Checking installed models…' : list.state === 'not_running' ? 'Ollama is not running, so installed models cannot be listed.' : 'No installed models were found.'}</p>
          )}
          <details>
            <summary>Advanced: enter an installed model name</summary>
            <label className="field"><span>Model name</span><input value={manual} maxLength={100} onChange={(e) => setManual(e.target.value.trim())} /></label>
            <button disabled={busy || !manual} onClick={() => void save(manual)}>Use this model</button>
          </details>
          </>}
          <details><summary>Advanced date and time settings</summary>
          <label className="field"><span>Time zone for “Friday”, “tomorrow” and clock times</span>
            <input value={zone} maxLength={64} onChange={(e) => setZone(e.target.value.trim())} />
          </label>
          <button disabled={busy || !zone || zone === cur.date_zone} onClick={() => void save(undefined, zone)}>Save time zone</button>
          </details>
        </>
      )}
      {msg && <p role="status">{msg}</p>}
    </section>
  );
}

type Preference = {value:string;revision:number};
type Loaded = {status:AppStatus;health:Health;theme:Preference;accent:Preference;text_size:Preference;motion:Preference};

function MessageRecovery() {
  const [data,setData]=useState<DailySummary|null>(null);
  const [error,setError]=useState<string|null>(null);
  const [busy,setBusy]=useState(false);
  const [notice,setNotice]=useState('');
  const load=useCallback(async()=>{try{setData(await api.autoSummary(0,1));setError(null);}catch(e){setError(asIpcError(e).message);}},[]);
  useEffect(()=>{void load();},[load]);
  async function run(retry:boolean){if(busy)return;setBusy(true);try{if(retry)await api.autoQueueRetry(null);else await api.autoQueueCancel();setNotice(retry?'Stopped messages are queued to try again.':'Waiting analyses stopped. Your messages and sign-in are kept.');window.dispatchEvent(new Event('rei-assistant-changed'));await load();}catch(e){setError(asIpcError(e).message);}finally{setBusy(false);}}
  return <section aria-label="Message recovery"><p className="muted">Use these only when automatic reading gets stuck. For everyday Pause or Resume, use Settings → Connections.</p>
    {data && <><p>{data.queue.failed+data.queue.cancelled} stopped · {data.queue.queued+data.queue.running} waiting or reading</p><div className="actions">{data.queue.failed+data.queue.cancelled>0 && <button disabled={busy} onClick={()=>void run(true)}>Retry stopped messages</button>}{data.queue.queued+data.queue.running>0 && <button disabled={busy} onClick={()=>void run(false)}>Stop waiting analyses</button>}</div>{data.queue.failed+data.queue.cancelled+data.queue.queued+data.queue.running===0 && <p>Nothing needs repair right now.</p>}</>}
    {notice && <p role="status">{notice}</p>}{error && <p role="alert">{error} <button onClick={()=>void load()}>Try again</button></p>}
  </section>;
}

const TABS=[['connections','Connections'],['ai','AI'],['appearance','Appearance'],['sync','Sync & backup'],['advanced','Advanced']];
const settingsTab=()=>{const value=window.location.hash.startsWith('#/sync')?'sync':new URLSearchParams(window.location.hash.split('?')[1]).get('tab');return TABS.some(([v])=>v===value)?value!:'connections';};

export function SettingsScreen() {
  const [data,setData]=useState<Loaded|null>(null);
  const [busy,setBusy]=useState(false);
  const [tab,setTab]=useState(settingsTab);
  useEffect(()=>{const changed=()=>setTab(settingsTab());window.addEventListener('hashchange',changed);return()=>window.removeEventListener('hashchange',changed);},[]);
  const chooseTab=(value:string)=>{setTab(value);window.history.replaceState(null,'',`#/settings?tab=${value}`);};
  const [emailOpen,setEmailOpen]=useState(false);
  const [aiOpen,setAiOpen]=useState(false);
  const [recoveryOpen,setRecoveryOpen]=useState(false);
  const [error,setError]=useState<string|null>(null);
  const [saved,setSaved]=useState<string|null>(null);
  const load=useCallback(async()=>{
    try{
      const [status,health,theme,accent,textSize,motion]=await Promise.all([api.appStatus(),api.health(),api.settingsGet('theme'),api.settingsGet('accent'),api.settingsGet('text_size'),api.settingsGet('motion')]);
      const pref=(value:string,revision?:number):Preference=>({value,revision:revision??0});
      const next={status,health,theme:pref(resolveTheme(theme?.value),theme?.revision),accent:pref(resolveAccent(accent?.value),accent?.revision),text_size:pref(['small','large'].includes(String(textSize?.value))?String(textSize?.value):'standard',textSize?.revision),motion:pref(motion?.value==='reduced'?'reduced':'system',motion?.revision)};
      applyTheme(next.theme.value);applyAccent(next.accent.value);applyTextSize(next.text_size.value);applyMotion(next.motion.value);setData(next);setError(null);
    }catch(e){setError(asIpcError(e).message);}
  },[]);
  useEffect(()=>{void load();},[load]);
  async function change(key:'theme'|'accent'|'text_size'|'motion',value:string){
    if(!data || busy)return;setBusy(true);setSaved(null);
    try{const result=await api.settingsSet(key as SettingKey,value,data[key].revision);const apply={theme:applyTheme,accent:applyAccent,text_size:applyTextSize,motion:applyMotion};apply[key](value);setData({...data,[key]:{value,revision:result.revision}});setError(null);setSaved(key==='theme'?s.settings.themeSaved(s.settings.themes[value]):key==='accent'?'Colour saved.':key==='text_size'?'Text size saved.':'Animation preference saved.');}
    catch(e){const err=asIpcError(e);if(err.code==='Conflict'){await load();setError(s.settings.conflict);}else setError(err.message);}
    finally{setBusy(false);}
  }
  if(!data)return error?<section className="card" role="alert"><p>{s.settings.loadFailed(error)}</p><button onClick={()=>void load()}>{s.retry}</button></section>:<p role="status">{s.loading}</p>;
  return <>
    <div className="settings-tabs" role="tablist" aria-label="Settings sections">{TABS.map(([value,label])=><button key={value} id={`settings-tab-${value}`} role="tab" aria-selected={tab===value} aria-controls="settings-panel" tabIndex={tab===value?0:-1} onKeyDown={e=>{const values=TABS.map(([v])=>v);const index=values.indexOf(tab);if(['ArrowLeft','ArrowRight','Home','End'].includes(e.key)){e.preventDefault();const next=e.key==='Home'?0:e.key==='End'?values.length-1:(index+(e.key==='ArrowRight'?1:values.length-1))%values.length;chooseTab(values[next]);document.getElementById(`settings-tab-${values[next]}`)?.focus();}}} onClick={()=>chooseTab(value)}>{label}</button>)}</div>
    <div role="tabpanel" id="settings-panel" aria-labelledby={`settings-tab-${tab}`}>
    {tab==='connections' && <><AutomaticInbox onAdvanced={()=>{chooseTab('ai');}}/>
      <details className="settings-disclosure" open={emailOpen} onToggle={e=>setEmailOpen(e.currentTarget.open)}><summary><strong>Email preferences</strong><span>Choose mail folders or how often to check.</span></summary>{emailOpen && <ConnectionsPanel showTeams={false} compact onChanged={()=>{window.dispatchEvent(new Event('rei-assistant-changed'));}}/>}</details>
      <p className="integration-roadmap">Sources · HIZ email. Microsoft Power Apps is planned; it is not connected yet.</p></>}
    {tab==='ai' && <AiSettings localControls={<LocalAiSettings/>}/>}
    {tab==='appearance' && <>
    <section className="card appearance-card" aria-label={s.settings.controls}><span className="overline">MAKE IT YOURS</span><h2>Appearance</h2><p className="muted">A calm workspace, in your colours.</p>
      {error && <p role="alert">{error}</p>}
      <label className="field"><span>{s.settings.theme}</span><select disabled={busy} value={data.theme.value} onChange={e=>void change('theme',e.target.value)}>{Object.entries(s.settings.themes).map(([v,l])=><option key={v} value={v}>{l}</option>)}</select></label>
      <div role="group" aria-label="Accent colour" className="accent-options">{ACCENTS.map(a=><button key={a.value} disabled={busy} aria-pressed={data.accent.value===a.value} onClick={()=>void change('accent',a.value)}><span className="accent-swatch" style={{background:a.color}}/>{a.name}</button>)}</div>
      <div className="appearance-preview" aria-hidden="true"><div/><span/><span/><small>Your day, gently organised.</small></div>
      <div className="reading-preferences"><label className="field"><span>Text size</span><select disabled={busy} value={data.text_size.value} onChange={e=>void change('text_size',e.target.value)}><option value="small">Small</option><option value="standard">Standard</option><option value="large">Large</option></select></label>
      <label className="motion-choice"><input type="checkbox" disabled={busy} checked={data.motion.value==='reduced'} onChange={e=>void change('motion',e.target.checked?'reduced':'system')}/>Reduce animations</label><p className="muted">Your device’s reduced-motion preference is always respected.</p></div>
      <p className="settings-language">Language · English</p>{saved && <p role="status" className="success">{saved}</p>}
    </section>
    </>}
    {tab==='sync' && <section className="card"><h2>Sync &amp; backup</h2><p role="status">Sync and backup are not available in this version. Your messages, tasks and preferences stay on this computer.</p><p className="muted">No cloud account or subscription is connected.</p></section>}
    {tab==='advanced' && <section className="card advanced-settings"><h2>Advanced settings</h2><div className="settings-more"><p className="muted">Optional controls for local AI and troubleshooting.</p>
      <details className="settings-disclosure" open={aiOpen} onToggle={e=>setAiOpen(e.currentTarget.open)}><summary><strong>AI model &amp; time zone</strong><span>Change the local model or how dates are understood.</span></summary>{aiOpen && <LocalAiSettings/>}</details>
      <details className="settings-disclosure" open={recoveryOpen} onToggle={e=>setRecoveryOpen(e.currentTarget.open)}><summary><strong>Message recovery</strong><span>Retry reading that stopped, without reconnecting email.</span></summary>{recoveryOpen && <MessageRecovery/>}</details>
      <details className="settings-disclosure diagnostics"><summary><strong>{s.settings.diagnostics}</strong><span>Technical information, only for troubleshooting.</span></summary><dl><dt>{s.settings.database}</dt><dd>{data.health.database_ok?s.settings.dbOk(data.status.schema_version):s.settings.dbBad}</dd><dt>{s.settings.ipc}</dt><dd>{data.status.ipc_version}</dd><dt>{s.settings.device}</dt><dd>{data.status.device_id}</dd><dt>{s.settings.lock}</dt><dd>{data.status.lock_state}</dd></dl></details>

    </div></section>}
    </div>
  </>;
}
