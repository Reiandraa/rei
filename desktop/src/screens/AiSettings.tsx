import { useEffect, useState, type ReactNode } from 'react';
import { api, aiName, asIpcError, type AiProvider, type AiRequest, type AiSettings as Settings } from '../ipc';
import { DeleteAction } from '../components/DeleteAction';

const names={local:'Local AI',openai:'OpenAI',anthropic:'Claude / Anthropic'};
export function AiSettings({localControls}:{localControls:ReactNode}) {
  const [data,setData]=useState<Settings|null>(null);
  const [provider,setProvider]=useState<AiProvider>('local');
  const [key,setKey]=useState('');const [model,setModel]=useState('');
  const [models,setModels]=useState<string[]>([]);const [consent,setConsent]=useState(false);
  const [limit,setLimit]=useState(50);const [busy,setBusy]=useState(false);
  const [notice,setNotice]=useState('');const [error,setError]=useState('');
  const load=async()=>{try{const d=await api.aiSettings();setData(d);setProvider(d.provider);setModel(d.provider==='local'?'':d.model);setLimit(d.daily_limit);setError('');}catch(e){setError(asIpcError(e).message);}};
  useEffect(()=>{void load();},[]);
  const change=async(req:AiRequest,base=data)=>{if(!base)throw new Error('Reload AI settings first.');const d=await api.aiSettings({...req,expected_revision:base.revision});setData(d);return d;};
  async function check(){if(!data||provider==='local'||busy)return;setBusy(true);setError('');setNotice('');const entered=key;setKey('');
    try{let d=data;if(entered)d=await change({action:'save_key',provider,api_key:entered},d);d=await change({action:'test',provider},d);setModels(d.available_models);setNotice('API key accepted. Choose a text model, then enable API use below. No messages were sent by this check.');}
    catch(e){setError(asIpcError(e).message);}finally{setBusy(false);}}
  async function activate(){if(!data||busy)return;setBusy(true);setError('');setNotice('');
    try{const d=await change(provider==='local'?{action:'use_local',model:data.local_model}:{action:'use_api',provider,model,consent,daily_limit:limit});setConsent(false);setNotice(d.provider==='local'?'Local AI is active.':'API is active for analysis, summaries and calendar labels while REI is open.');window.dispatchEvent(new Event('rei-assistant-changed'));}
    catch(e){setError(asIpcError(e).message);}finally{setBusy(false);}}
  const saved=data && (provider==='openai'?data.openai_saved:provider==='anthropic'?data.anthropic_saved:false);
  return <section className="ai-settings" aria-label="AI settings">
    <header><span className="overline">HOW REI READS</span><h2>Your AI, your choice</h2><p className="muted">Use a model on this computer, or connect your own API account.</p></header>
    {error && <p role="alert">{error} <button disabled={busy} onClick={()=>void load()}>Reload AI settings</button></p>}
    {!data?<p role="status">Loading AI settings…</p>:<>
      <p className="ai-active">Active · {data.provider==='local'?'Local Ollama':aiName(`api_${data.provider}/${data.model}`)} <span className="muted">{data.model}</span></p>
      <div className="ai-provider-options" role="group" aria-label="AI provider">{(Object.keys(names) as AiProvider[]).map(p=><button key={p} disabled={busy} aria-pressed={provider===p} onClick={()=>{setProvider(p);setKey('');setModel(p===data.provider?data.model:'');setModels([]);setConsent(false);setNotice('');setError('');}}><strong>{names[p]}</strong><small>{p==='local'?'On this computer · no API bill':'Your API account · usage billed separately'}</small></button>)}</div>
      {provider==='local'?<div className="ai-setup"><h3>Keep your messages on this computer</h3><p className="muted">Uses Ollama and your installed model. Performance depends on the computer.</p>{data.provider==='local'?localControls:<button className="primary" disabled={busy} onClick={()=>void activate()}>Use local AI</button>}</div>:<div className="ai-setup">
        <h3>Connect {names[provider]}</h3><p className="muted">Use an API key from your own provider account. A chat subscription does not connect the API.</p>
        <div className="ai-key-row"><label className="field"><span>API key</span><input type="password" autoComplete="off" spellCheck={false} value={key} maxLength={512} placeholder={saved?'Key saved securely · enter only to replace':'Paste your API key'} onChange={e=>setKey(e.target.value)} disabled={busy}/></label><button disabled={busy||(!key&&!saved)} onClick={()=>void check()}>{busy?'Working…':'Check API key'}</button></div>
        <p className="muted">Saved in this computer’s secure storage. This check requests the model list; it does not generate text or read your email.</p>
        <label className="field"><span>API model</span><input list="api-text-models" value={model} maxLength={85} placeholder="Choose or enter a text model ID" disabled={busy} onChange={e=>{setModel(e.target.value.trim());setConsent(false);}}/><datalist id="api-text-models">{models.map(m=><option key={m} value={m}/>)}</datalist></label>
        <details className="ai-usage"><summary>Usage controls · {data.used_today} reserved today</summary><label className="field"><span>Daily API request limit</span><input type="number" min={1} max={500} value={limit} disabled={busy} onChange={e=>setLimit(Number(e.target.value))}/></label><p className="muted">Resets at 00:00 UTC. Includes analysis, summaries and calendar labels across both providers. At most 4,096 output tokens per request. Requests are reserved before sending, including uncertain failures. This limits requests, not a dollar amount; set billing limits with your provider.</p></details>
        <label className="ai-consent"><input type="checkbox" checked={consent} disabled={busy} onChange={e=>setConsent(e.target.checked)}/><span>I allow REI to send message text, saved preferences and calendar titles to {names[provider]} for automatic AI processing, billed to my API account.</span></label>
        <p className="muted">Changing provider or model may rebuild summaries for up to 100 stored messages. Message and calendar content is sent only while API use is enabled. Tasks still wait for your approval.</p>
        <div className="actions"><button className="primary" disabled={busy||!saved||!model||!consent||!Number.isInteger(limit)||limit<1||limit>500} onClick={()=>void activate()}>Enable {provider==='openai'?'OpenAI':'Claude'} API</button>{saved && <DeleteAction title={`${names[provider]} API key`} kind="API key" disabled={busy} onConfirm={async()=>{setBusy(true);try{await change({action:'delete_key',provider,consent:true});setKey('');setConsent(false);setNotice('API key removed. If it was active, REI has switched to local AI.');window.dispatchEvent(new Event('rei-assistant-changed'));}catch(e){setError(asIpcError(e).message);}finally{setBusy(false);}}}/>}</div>
      </div>}
    </>}
    {notice && <p role="status" className="success">{notice}</p>}
  </section>;
}
