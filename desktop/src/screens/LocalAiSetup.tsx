import { useCallback, useEffect, useRef, useState } from 'react';
import { api, asIpcError, type LocalSetup } from '../ipc';
import './LocalAiSetup.css';

const catalog = [
  { name: 'qwen3.5:2b', label: 'Light', size: 'About 3 GB', detail: 'A smaller starting point for limited memory.' },
  { name: 'qwen3.5:4b', label: 'Balanced', size: 'About 4 GB', detail: 'A middle ground for memory and capability.' },
  { name: 'qwen3.5:9b', label: 'Larger', size: 'About 7 GB', detail: 'Needs more memory and can take longer.' },
];
type ModelList = {state: string; models: string[]};
type Props = {list: ModelList | null; current: string; onRefresh: () => Promise<void>; onSave: (model: string) => Promise<void>; busy: boolean};

export function LocalAiSetup({list, current, onRefresh, onSave, busy}: Props) {
  const [setup, setSetup] = useState<LocalSetup | null>(null);
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const [working, setWorking] = useState(false);
  const [choice, setChoice] = useState('');
  const [showDownloads, setShowDownloads] = useState(false);
  const alive = useRef(true);
  const checking = useRef(false);
  const selected = useRef(false);
  const inspect = useCallback(async () => {
    if (checking.current) return;
    checking.current = true;
    try {
      const result = await api.localAiSetup('inspect');
      if (alive.current) {
        setSetup(result);
        if (!selected.current) setChoice(result.recommended_model);
      }
    } catch { /* Older backends can still list installed models and show manual recovery. */ }
    finally { checking.current = false; }
  }, []);
  useEffect(() => {
    alive.current = true;
    void inspect();
    const refresh = () => { if (!document.hidden) { void inspect(); void onRefresh(); } };
    window.addEventListener('focus', refresh);
    document.addEventListener('visibilitychange', refresh);
    const timer = window.setInterval(refresh, 4000);
    return () => { alive.current = false; clearInterval(timer); window.removeEventListener('focus', refresh); document.removeEventListener('visibilitychange', refresh); };
  }, [inspect, onRefresh]);
  const downloading = setup?.download.state === 'downloading' || setup?.download.state === 'cancelling';
  const ready = list?.state === 'ready';
  const models = list?.models ?? [];
  const hasCurrent = ready && models.includes(current);
  async function act(action: 'installer' | 'open' | 'download' | 'cancel') {
    if (working) return;
    setWorking(true); setError(''); setNotice('');
    try {
      const result = await api.localAiSetup(action, action === 'download' ? choice : undefined);
      if (!alive.current) return;
      setSetup(result);
      setNotice(action === 'installer' ? 'The official download page is open. Install Ollama, then return here. REI checks again automatically.' : action === 'open' ? 'Opening Ollama. REI will check for it automatically.' : '');
      await onRefresh();
    } catch(e) { if (alive.current) setError(asIpcError(e).message); }
    finally { if (alive.current) setWorking(false); }
  }
  const step = !ready ? 1 : models.length === 0 ? 2 : hasCurrent ? 3 : 2;
  const download = setup?.download;
  return <div className="local-ai-setup">
    <ol className="local-setup-steps" aria-label="Local AI setup progress">{['Connect Ollama', 'Choose a model', 'Ready'].map((label, i) => <li key={label} className={step > i + 1 ? 'complete' : step === i + 1 ? 'active' : ''} aria-current={step === i + 1 ? 'step' : undefined}><span aria-hidden="true">{step > i + 1 ? '✓' : i + 1}</span>{label}</li>)}</ol>
    <p className={`local-setup-status ${hasCurrent ? 'success' : ''}`} role="status">{!list ? 'Checking local AI…' : hasCurrent ? `Ready · ${current}` : ready ? models.length > 0 ? 'Ollama is connected. Choose an installed model to finish.' : 'Ollama is connected. Download a model to get started.' : list.state === 'error' ? 'Ollama could not be checked. Open it and try again.' : setup?.installed === false ? 'Install Ollama to use AI on this computer.' : 'Ollama is not running. Open it to continue.'}</p>
    {!ready && <div className="local-setup-instructions">
      <h4>{setup?.installed ? 'Open Ollama' : 'Set up Ollama once'}</h4>
      <p>Ollama runs local AI for REI. Your planner stays available while you set it up.</p>
      <div className="actions">
        {setup?.installed ? <button className="primary" disabled={working} onClick={() => void act('open')}>Open Ollama</button> : <button className="primary" disabled={working} onClick={() => void act('installer')}>Install Ollama</button>}
        {!setup?.installed && <button disabled={working} onClick={() => void act('open')}>Already installed? Open Ollama</button>}
        <button disabled={working || busy} onClick={() => {setError(''); void inspect(); void onRefresh();}}>Check again</button>
      </div>
      <p className="muted">{setup?.platform === 'macos' ? 'Open the downloaded installer, drag Ollama into Applications, then open Ollama. Return to REI when it is running.' : setup?.platform === 'windows' ? 'Open the downloaded installer and follow its steps. Open Ollama from Start, then return to REI.' : 'Install Ollama from its official download page and open it, then return here.'} No Terminal commands are needed for this setup.</p>
      <details><summary>Still cannot connect?</summary><p>Open Ollama from your applications and keep it running. If it is already open, restart it and choose Check again. Update Ollama if model downloads are rejected.</p><a href="https://ollama.com/download" target="_blank" rel="noreferrer">Official Ollama download</a></details>
    </div>}
    {ready && models.length > 0 && <><label className="field"><span>Installed model</span><select disabled={busy} value={hasCurrent ? current : ''} onChange={e => e.target.value && void onSave(e.target.value)}>{!hasCurrent && <option value="">Choose a model</option>}{models.map(m => <option key={m} value={m}>{m}</option>)}</select></label><p className="muted">{hasCurrent ? 'This model is installed and selected. Summaries can run when automatic assistance is enabled; speed and summary quality depend on the model and computer.' : `Saved choice: ${current}. This model is not installed here yet.`}</p></>}
    {ready && models.length > 0 && <button className="local-download-toggle" aria-expanded={showDownloads} onClick={() => setShowDownloads(v => !v)}>{showDownloads ? 'Hide model downloads' : 'Download another model'}</button>}
    {ready && (models.length === 0 || showDownloads || downloading || download?.state === 'complete') && <section className="local-model-download" aria-label="Download a local model">
      <h4>{models.length === 0 ? 'Choose your first model' : 'Get another local model'}</h4>
      <p className="muted">{setup?.memory_gb ? `${setup.memory_gb} GB RAM detected. ` : ''}The recommendation is a starting point based on system RAM, not a GPU benchmark. Windows GPU memory and other open apps also matter. Smaller models may miss details.</p>
      <label className="field"><span>Model to download</span><select value={choice} disabled={working || downloading} onChange={e => {selected.current = true; setChoice(e.target.value);}}><option value="">Choose a model</option>{catalog.map(m => <option key={m.name} value={m.name}>{m.label} · {m.name} · {m.size}{setup?.recommended_model === m.name ? ' · Suggested' : ''}</option>)}</select></label>
      <p className="muted">{catalog.find(m => m.name === choice)?.detail} Download sizes are approximate and can change. Allow extra disk space for installation. Downloads use your internet connection and stay on this computer.</p>
      {downloading ? <div className="model-download-progress" role="status"><strong>{download?.model}</strong><p>{download?.detail}</p>{download && download.total > 0 && <><progress aria-label="Current model file download" value={download.completed} max={download.total}/><small>Current file · {(download.completed / 1e9).toFixed(2)} / {(download.total / 1e9).toFixed(2)} GB</small></>}<button disabled={working || download?.state === 'cancelling'} onClick={() => void act('cancel')}>{download?.state === 'cancelling' ? 'Stopping…' : 'Cancel download'}</button><p className="muted">You can keep using REI. Ollama must stay open. Cancelling may take a few seconds; downloaded files may remain for a later retry.</p></div> : <>
        {download?.state && download.state !== 'idle' && <p role={download.state === 'error' ? 'alert' : 'status'}>{download.detail}</p>}
        {download?.state === 'complete' && models.includes(download.model) && current !== download.model ? <button className="primary" disabled={busy} onClick={() => void onSave(download.model)}>Use {download.model}</button> : <button className="primary" disabled={working || !choice || !setup || models.includes(choice)} onClick={() => void act('download')}>{models.includes(choice) ? 'Already installed' : 'Download model'}</button>}
      </>}
      <p className="muted">REI only downloads after you choose Download model. No API key or API charge is involved.</p>
    </section>}
    {notice && <p role="status">{notice}</p>}{error && <p role="alert">{error}</p>}
    {!setup && <p className="muted">Device setup information is unavailable. You can install/open Ollama from your applications and choose Check again. <a href="https://ollama.com/download" target="_blank" rel="noreferrer">Official download</a></p>}
  </div>;
}
