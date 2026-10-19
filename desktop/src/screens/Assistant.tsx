import { useCallback, useEffect, useRef, useState } from 'react';
import { api, asIpcError, aiName, isApiModel, modelName, type AssistantBrief, type AssistantNote, type BriefEntry } from '../ipc';
import { DeleteAction } from '../components/DeleteAction';

const when=(value:string|null)=>value?new Date(value).toLocaleString('en-GB',{dateStyle:'medium',timeStyle:'short'}):'Not summarized yet';
function status(d:AssistantBrief) {
  if(d.state==='off')return 'Automatic assistance is paused or off. Saved summaries are still available.';
  if(d.state==='offline')return d.detail || 'Local AI is unavailable. Saved summaries are kept.';
  if(!d.enabled)return 'Choose Summarize to prepare your important-message briefing.';
  if(d.pending>0)return `${d.state==='running'?(isApiModel(d.model)?`Summarizing with ${aiName(d.model)}`:'Summarizing locally'):(isApiModel(d.model)?'Waiting for AI':'Waiting for local AI')} · ${d.pending} message${d.pending===1?'':'s'} remaining. You can keep using REI.`;
  return 'Up to date with the messages in this briefing.';
}
function useBrief() {
  const [data,setData]=useState<AssistantBrief|null>(null);
  const [error,setError]=useState<string|null>(null);
  const version=useRef(0);
  const invalidate=useCallback(()=>{version.current++;},[]);
  const load=useCallback(async(refresh=false)=>{
    const seq=++version.current;
    try{const next=refresh?await api.assistantSummaryRefresh():await api.assistantSummaryGet();if(seq===version.current){setData(next);setError(null);}}
    catch(e){if(seq===version.current)setError(asIpcError(e).message);}
  },[]);
  useEffect(()=>{void load();const timer=setInterval(()=>void load(),10000);const changed=()=>void load();window.addEventListener('rei-assistant-changed',changed);return()=>{invalidate();clearInterval(timer);window.removeEventListener('rei-assistant-changed',changed);};},[load,invalidate]);
  return {data,error,load};
}
function Entry({entry}:{entry:BriefEntry}) {
  return <article className="brief-entry"><div className="section-head"><strong>{entry.subject}</strong><time>{when(entry.received_at)}</time></div>
    {entry.points.length>0 && <ul>{entry.points.map((p,n)=><li key={n}>{p.text}</li>)}</ul>}
    {entry.detail && <p className="muted">{entry.detail}</p>}
    <div className="brief-entry-actions"><a href={`#/inbox?source=${encodeURIComponent(entry.source_id)}`}>View message →</a>{entry.points.length>0 && <details><summary>Source quote</summary>{entry.points.map((p,n)=><blockquote key={n}>{p.excerpt}</blockquote>)}</details>}</div>
  </article>;
}
/** Shared cached information preview; task/schedule approvals stay in Daily review. */
export function ImportantBriefPreview() {
  const {data}=useBrief();
  const important=data?.entries.filter(e=>e.importance==='important')??[];
  const points=important.slice(0,3);
  const more=important.length-points.length;
  return <div className="daily-brief-preview" aria-label="Important message summary"><div className="section-head"><h3>Important updates</h3><a href={data?.enabled?"#/assistant":"#/assistant?summarize=1"}>{points.length?'Open summary →':'Summarize messages →'}</a></div>
    {points.length>0?<ul>{points.map(e=><li key={e.source_id}>{e.priority && e.priority.rank<3 && <small className="brief-priority">{e.priority.reason} · </small>}{e.points[0]?.text??e.subject} <a href={`#/inbox?source=${encodeURIComponent(e.source_id)}`}>View message</a></li>)}</ul>:<p className="muted">{data?.pending?'New messages are waiting to be summarized.':data?.enabled?'No important points in the completed summaries.':'Your important-message briefing lives in Assistant.'}</p>}
    {more>0 && <p className="brief-more"><a href="#/assistant">{more} more important update{more===1?'':'s'} →</a></p>}
    {data && <small>{data.pending>0?`${data.pending} still to summarize · `:''}{data.entries.filter(e=>e.importance!=='important'||e.partial).length>0?'Some messages need attention · ':''}Updated {when(data.updated_at)}</small>}
  </div>;
}
function Preferences() {
  const [notes,setNotes]=useState<AssistantNote[]>([]);
  const [open,setOpen]=useState(false);
  const [editing,setEditing]=useState<AssistantNote|null>(null);
  const [text,setText]=useState('');
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string|null>(null);
  const load=async()=>{try{setNotes(await api.assistantMemory('list'));setError(null);}catch(e){setError(asIpcError(e).message);}};
  const save=async()=>{if(busy || !text.trim())return;setBusy(true);try{setNotes(await api.assistantMemory('save',{id:editing?.id,text:text.trim(),expected_hash:editing?.hash,confirm:true}));setText('');setEditing(null);setError(null);window.dispatchEvent(new Event('rei-assistant-changed'));}catch(e){setError(asIpcError(e).message);}finally{setBusy(false);}};
  return <details className="assistant-preferences" open={open} onToggle={e=>{const next=e.currentTarget.open;setOpen(next);if(next)void load();}}><summary>What REI remembers</summary>{open && <>
    <p className="muted">Only preferences you save here. For example, “I study computer science; prioritize HM1 and Spanish classes.” They help tailor your summaries. Email is not automatically saved as personal memory.</p>
    <ul className="rows">{notes.map(note=><li key={note.id}><span className="row-main">{note.text}</span><button disabled={busy} onClick={()=>{setEditing(note);setText(note.text);}}>Edit</button><DeleteAction title={note.text} kind="preference" disabled={busy} onConfirm={async()=>{setNotes(await api.assistantMemory('delete',{id:note.id,expected_hash:note.hash,confirm:true}));if(editing?.id===note.id){setEditing(null);setText('');}window.dispatchEvent(new Event('rei-assistant-changed'));}}/></li>)}</ul>
    <form onSubmit={e=>{e.preventDefault();void save();}}><label>{editing?'Edit preference':'Add a preference'}<textarea value={text} maxLength={500} rows={2} onChange={e=>setText(e.target.value)} disabled={busy}/></label><p className="muted">Saved locally until you edit or delete it. Changing preferences may recheck message relevance.</p><div className="actions"><button className="primary" disabled={busy || !text.trim()}>{busy?'Saving…':editing?'Save changes':'Save preference'}</button>{editing && <button type="button" onClick={()=>{setEditing(null);setText('');}}>Cancel</button>}</div></form>
    {error && <p role="alert">{error} <button onClick={()=>void load()}>Reload preferences</button></p>}
  </>}</details>;
}
export function AssistantScreen() {
  const {data,error,load}=useBrief();
  const [requesting,setRequesting]=useState(false);
  const started=useRef(false);
  useEffect(()=>{if(!started.current && new URLSearchParams(window.location.hash.split('?')[1]).get('summarize')==='1'){started.current=true;void load(true);window.history.replaceState(null,'','#/assistant');}},[load]);
  const important=data?.entries.filter(e=>e.importance==='important')??[];
  const attention=data?.entries.filter(e=>e.importance!=='important')??[];
  return <>
    <div className="page-heading"><div><p className="muted">What matters from your messages, with the original source beside every point.</p></div><button className="primary" disabled={requesting || data?.state==='running' || data?.state==='queued'} onClick={()=>{setRequesting(true);void load(true).finally(()=>setRequesting(false));}}>{requesting?'Starting…':data?.state==='running'||data?.state==='queued'?'Summarizing…':'Summarize important updates'}</button></div>
    {error && <p role="alert">{error} <button onClick={()=>void load()}>Try again</button></p>}
    {!data?<p role="status">Loading saved summary…</p>:<>
      <div className="brief-status" role="status"><span className={data.pending?'working-dot':'connection-dot'}/><div><strong>{status(data)}</strong><small>{when(data.updated_at)} · {aiName(data.model)} · {modelName(data.model)}</small></div>{data.state==='off' && <a href="#/settings?tab=connections">Open connections →</a>}</div>
      <p className="muted brief-coverage">Latest {Math.min(data.total,data.scope_limit)} of {data.total} stored messages · {data.routine} routine messages kept out of this briefing · {data.pending} not summarized yet. Older messages remain in Messages.</p>
      <section className="brief-section" aria-label="Important updates"><div className="section-head"><h2>Important</h2><a href="#/today">Review suggested tasks on Today →</a></div>{important.length?important.map(entry=><Entry key={entry.source_id} entry={entry}/>):<p className="empty-state">{data.pending?'Your important updates will appear as messages are summarized.':'No important updates in the completed summaries.'}</p>}</section>
      {attention.length>0 && <section className="brief-section" aria-label="Messages needing attention"><h2>Needs your attention</h2><p className="muted">Unclear or incomplete messages are kept here so you can check them.</p>{attention.map(entry=><Entry key={entry.source_id} entry={entry}/>)}</section>}
      <p className="pane-note">Summary points are information, not approved tasks. Review new tasks and schedule changes together on Today.</p>
    </>}
    <Preferences/>
  </>;
}
