// Academic inbox: stored university messages (original German text, read-only), local
// analysis and review of cited suggestions. HIZ/Teams accounts are set up in the Connections panel.
import { MessageReader } from '../components/MessageReader';
import { isScheduleNotice } from '../components/ScheduleNotice';
import { useCallback, useEffect, useState } from 'react';
import { api, asIpcError, sentDay, isApiModel, aiName, type Candidate, type InboxOverview, type SourceDetail, type SourceSummary } from '../ipc';

const CHANNEL: Record<string, string> = {
  hiz_email: 'Email', teams_chat: 'Teams chats', teams_channel: 'Teams channels', manual: 'Pasted by you',
};
const STATE: Record<string, string> = {
  not_configured: 'Not set up', consent_needed: 'Waiting for your permission',
  testing: 'Testing', connected: 'Connected', degraded: 'Problem fetching', expired: 'Sign-in expired', permission_denied: 'Access denied',
};
const ANALYSIS: Record<string, string> = {
  unanalysed: 'Not analysed yet', outdated: 'Changed since last analysis', candidates: 'Has suggestions',
  routine: 'Routine — nothing to do', nothing_found: 'Analysed — nothing found', invalid_output: 'Analysis unusable',
};
const PAGE = 50;

function when(iso: string | null) {
  return iso ? new Date(iso).toLocaleString('en-GB', { dateStyle: 'medium', timeStyle: 'short' }) : 'never';
}

export function ConnectionList({ ov }: { ov: InboxOverview }) {
  return (
    <ul aria-label="Connections">
      {ov.connections.filter((c) => c.channel === 'hiz_email' || (c.channel === 'manual' && c.accounts > 0)).map((c) => (
        <li key={c.channel}>
          <strong>{CHANNEL[c.channel]}</strong>: {STATE[c.state] ?? c.state}
          {c.accounts > 0 && <> · last fetch {when(c.last_fetch_at)} · {c.sources} stored</>}
          {c.state === 'connected' && c.sources === 0 && <> · no messages yet</>}
          {c.last_error && <span className="muted"> · {c.last_error}</span>}
        </li>
      ))}
    </ul>
  );
}

export function CandidateCard({ c, onDone }: { c: Candidate; onDone: () => Promise<void> }) {
  const [title, setTitle] = useState(c.title);
  const [due, setDue] = useState(c.due_date ?? '');
  const [dueTime, setDueTime] = useState(c.due_time ?? '');
  const [msg, setMsg] = useState<string | null>(null);
  const reviewable = c.status === 'pending' && !c.expired && c.source_state === 'current';
  const dateChanged = due !== (c.due_date ?? '') || dueTime !== (c.due_time ?? '');
  const needsDate = c.date_status === 'ambiguous';
  const run = async (f: () => Promise<unknown>, ok: string) => {
    try { await f(); setMsg(ok); await onDone(); } catch (e) { setMsg(asIpcError(e).message); }
  };
  return (
    <article className="card" aria-label={`Suggestion: ${c.title}`}>
      <p><strong>{c.schedule_active !== undefined && c.schedule_active !== null ? c.schedule_active ? 'Schedule updated' : 'Schedule change undone' : isScheduleNotice(c) ? 'Schedule notice' : 'Suggestion (not a task yet)'}</strong> · {c.kind}{c.schedule_active == null ? ` · status ${c.status}` : ''}{c.expired ? ' · expired' : ''}</p>
      <blockquote lang="de">{c.excerpt}</blockquote>
      <p className="muted">
        Quote from “{c.source_subject ?? 'message'}” · <a href={`#/inbox?source=${c.source_id}`}>Open message</a> · version {c.source_hash.slice(0, 8)} ·
        model {c.model} (self-rated {Math.round(c.confidence * 100)}%, a rough hint only)
      </p>
      {c.source_state !== 'current' && <p role="alert">The original message {c.source_state === 'deleted' ? 'was deleted' : 'changed'}. This suggestion can no longer be approved.</p>}
      {c.conflict && <p role="alert">Another message gives a different date for this. Check both before approving.</p>}
      {c.date_status === 'explicit' && c.due_date && (
        <p className="when">{isScheduleNotice(c) ? 'Meeting date' : 'Due'} <strong>{c.due_date}{c.due_time ? `, ${c.due_time}` : ''}</strong>
          {c.date_basis === 'relative' ? ` · interpreted from “${c.date_text ?? ''}” and the send date ${sentDay(c)} (${c.date_zone ?? ''})` : ' · written in the message'}
        </p>
      )}
      {needsDate && !isScheduleNotice(c) && <p role="alert">The date is unclear{c.date_text ? ` (“${c.date_text}”)` : ''}. Enter it yourself or leave it empty for no date.</p>}
      {reviewable && isScheduleNotice(c) && <p>This message changes a schedule. <a href="#/today">Review it on Today</a> to update the affected routine.</p>}
      {reviewable && !isScheduleNotice(c) && (
        <div className="actions">
          <label>Title <input value={title} onChange={(e) => setTitle(e.target.value)} /></label>
          <label>Due date <input type="date" value={due} onChange={(e) => setDue(e.target.value)} /></label>
          <label>Due time <input type="time" value={dueTime} onChange={(e) => setDueTime(e.target.value)} /></label>
          <button className="primary" onClick={() => void run(() => api.candidateApprove(c, {
            ...(title !== c.title ? { title } : {}),
            ...(needsDate || dateChanged ? { date_resolved: true, due_date: due || null, ...(due && dueTime ? { due_time: dueTime } : {}) } : {}),
          }), 'Task created.')}>Create task</button>
          <button onClick={() => void run(() => api.candidateReject(c), 'Dismissed.')}>Dismiss</button>
        </div>
      )}
      {msg && <p role="status">{msg}</p>}
    </article>
  );
}

function Detail({ id, model, reload }: { id: string; model: string; reload: () => Promise<void> }) {
  const [d, setD] = useState<SourceDetail | null>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const load = useCallback(async () => { try { setD(await api.sourceGet(id)); } catch (e) { setMsg(asIpcError(e).message); } }, [id]);
  useEffect(() => { void load(); }, [load]);
  const analyse = async () => {
    setBusy(true); setMsg(null);
    try {
      const r = await api.sourceAnalyze(id, model);
      setMsg(r.status === 'invalid_output' ? `${isApiModel(model)?'The model':'The local model'} answer was not usable. Nothing was created.`
        : r.status === 'routine' ? 'Routine message — nothing to do.'
        : r.status === 'nothing_found' ? 'Nothing for your studies was found.'
        : `${r.created} new suggestion(s), ${r.already_known} already known.`);
      await Promise.all([load(), reload()]);
    } catch (e) {
      const err = asIpcError(e);
      setMsg(err.code === 'Offline' && !isApiModel(model) ? 'Ollama is not running on this computer. Start it and try again.' : err.message);
    } finally { setBusy(false); }
  };
  const done = useCallback(async () => { await Promise.all([load(), reload()]); }, [load, reload]);
  const cancel = async () => {
    try {
      const running = await api.sourceAnalyzeCancel();
      setMsg(running ? 'Cancellation requested. Waiting for the current request to finish.' : 'No analysis is running.');
    } catch (e) { setMsg(asIpcError(e).message); }
  };
  if (!d) return msg ? <p role="alert">{msg}</p> : <p role="status">Loading…</p>;
  return <section className="reader-content" aria-label="Message">
    <header className="reader-message-head"><h2 lang="de">{d.subject ?? '(no subject)'}</h2><p className="muted">{d.sender ?? 'Unknown sender'} · {when(d.received_at)}</p>
      <div className="reader-toolbar"><span className="message-status">{ANALYSIS[d.analysis] ?? d.analysis}</span><details className="manual-analysis"><summary>Analyse this message again</summary><div className="reader-analysis-menu"><p>Automatic reading handles your email. Use this to check this message again with your selected AI.</p><button className="primary" disabled={busy || d.deleted || !model} onClick={() => void analyse()}>{busy ? (isApiModel(model)?'Analysing…':'Analysing locally…') : isApiModel(model)?`Analyse with ${aiName(model)}`:'Analyse with local model'}</button>{busy && <button onClick={() => void cancel()}>Cancel</button>}</div></details></div>
      {msg && <p role="status">{msg}</p>}
    </header>
    <div className="reader-columns"><section className="reader-email" aria-label="Email text" tabIndex={0}><h3>Original message</h3><div className="source-text" lang="de" style={{whiteSpace:'pre-wrap'}}>{d.body_text}</div></section>
    <section className="reader-suggestions" aria-label="Message suggestions" tabIndex={0}><h3>Suggestions <span>{d.candidates.length}</span></h3><p className="muted">Prepared from this message. Nothing is added until you approve it.</p>{d.candidates.length===0 && <p className="empty-state">{d.analysis==='routine'?'Filed as routine. Nothing was deleted.':d.analysis==='unanalysed'?'Local reading has not finished yet.':'No suggestions from this message.'}</p>}{d.candidates.map(c=><CandidateCard key={c.id} c={c} onDone={done} />)}</section></div>
  </section>;
}

export function InboxScreen() {
  const [ov, setOv] = useState<InboxOverview | null>(null);
  const [list, setList] = useState<SourceSummary[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [sel, setSel] = useState<string | null>(() => new URLSearchParams(window.location.hash.split('?')[1]).get('source'));
  const [model, setModel] = useState('');
  useEffect(() => { api.autoSettingsGet().then(s => setModel(s.model), () => undefined); }, []);
  const [paste, setPaste] = useState({ title: '', text: '' });
  const [page, setPage] = useState(0);
  const [filter,setFilter] = useState('');
  const [adding,setAdding] = useState(false);
  const [importing,setImporting] = useState(false);
  const reload = useCallback(async () => {
    try { const [o, l] = await Promise.all([api.inboxOverview(), api.sourceList(PAGE, page * PAGE)]); setOv(o); setList(l); setErr(null); }
    catch (e) { setErr(asIpcError(e).message); }
  }, [page]);
  useEffect(() => { void reload(); }, [reload]);
  useEffect(() => {
    const onHash = () => setSel(new URLSearchParams(window.location.hash.split('?')[1]).get('source'));
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);
  const importPaste = async () => {
    if(importing)return;setImporting(true);setErr(null);
    try { const d = await api.sourceImportManual(paste.title, paste.text); setPaste({ title: '', text: '' }); setAdding(false); setSel(d.id); await reload(); }
    catch (e) { setErr(asIpcError(e).message); }
    finally {setImporting(false);}
  };
  const filtered=list.filter(m=>`${m.subject ?? ''} ${m.sender ?? ''}`.toLowerCase().includes(filter.toLowerCase()));
  const index=filtered.findIndex(m=>m.id===sel);
  return <>
    <div className="page-heading"><p className="muted">Your university email. Read a message beside its suggestions.</p><div className="actions"><a className="button primary" href="#/assistant?summarize=1">Summarize important updates</a><button className="secondary" onClick={()=>setAdding(!adding)}>+ Add a notice</button></div></div>
    {err && <p role="alert">{err}</p>}
    {adding && <form className="card manual-notice" aria-label="Add a notice manually" onSubmit={e=>{e.preventDefault();void importPaste();}}><h2>Add a notice manually</h2><p>For something outside your inbox: a class-group announcement, a campus webpage, or a letter. Copy its text here so REI can help you review it.</p><p className="muted">This adds a local copy to Messages. It does not send an email or connect another account.</p><label>Short title<input value={paste.title} onChange={e=>setPaste({...paste,title:e.target.value})} required /></label><label>Paste the notice<textarea rows={5} value={paste.text} onChange={e=>setPaste({...paste,text:e.target.value})} required /></label><div className="actions"><button type="submit" className="primary" disabled={importing || !paste.title.trim() || !paste.text.trim()}>{importing?'Adding…':'Add to Messages'}</button><button type="button" onClick={()=>setAdding(false)}>Cancel</button></div></form>}
    <section className="card message-list-card" aria-label="Messages"><div className="message-list-head"><h2>Messages</h2><label className="message-search"><span className="sr-only">Find in this page</span><input type="search" placeholder="Find a sender or subject…" value={filter} onChange={e=>setFilter(e.target.value)} /></label></div>
      {ov && <div className="message-pagination"><span>{ov.sources_total?`${page*PAGE+1}–${page*PAGE+list.length} of ${ov.sources_total} stored messages`:'No messages stored yet'}</span><div><button disabled={page===0} onClick={()=>{setPage(page-1);setFilter('');}}>Previous</button><button disabled={(page+1)*PAGE>=ov.sources_total} onClick={()=>{setPage(page+1);setFilter('');}}>Next</button></div></div>}
      {!filtered.length?<p className="empty-state">{filter?'No matching messages on this page.':'Your messages will appear here after the next email check.'}</p>:<ul className="message-list">{filtered.map(m=><li key={m.id}><button className="message-list-row" aria-label={`Open: ${m.subject ?? m.snippet}`} onClick={()=>setSel(m.id)}><span className="message-sender">{m.sender ?? 'Unknown sender'}</span><strong lang="de">{m.subject ?? '(no subject)'}</strong><span className="message-preview">{m.snippet}</span><span className="message-row-status">{ANALYSIS[m.analysis] ?? m.analysis}{m.pending_candidates>0?` · ${m.pending_candidates} to review`:''}</span><time>{when(m.received_at)}</time></button></li>)}</ul>}
    </section>
    {ov && <div className="message-connection-footer"><span>{`Email · ${STATE[ov.connections.find(c=>c.channel==='hiz_email')?.state??'not_configured']??'Not connected'}`} · Last check {when(ov.connections.find(c=>c.channel==='hiz_email')?.last_fetch_at??null)}</span><a href="#/settings?tab=connections">Connection settings →</a></div>}
    {ov?.connections.filter(c=>c.channel==='hiz_email' && c.last_error).map(c=><p key={c.channel} role="alert">Email needs attention: {c.last_error} <a href="#/settings?tab=connections">Open connections</a></p>)}
    {sel && <MessageReader onClose={()=>{setSel(null);if(window.location.hash.includes('?source='))window.history.replaceState(null,'','#/inbox');}} onPrevious={index>0?()=>setSel(filtered[index-1].id):undefined} onNext={index>=0 && index<filtered.length-1?()=>setSel(filtered[index+1].id):undefined}><Detail key={sel} id={sel} model={model} reload={reload} /></MessageReader>}
  </>;
}
