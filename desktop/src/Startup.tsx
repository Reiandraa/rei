import { Component, useEffect, useState, type ReactNode } from 'react';
import { api, type AiSettings } from './ipc';
import { Icon } from './components/Icon';
import { applyAccent, applyMotion, applyTextSize, applyTheme } from './theme';

function within<T>(request: Promise<T>, milliseconds: number): Promise<T> {
  let timeout: ReturnType<typeof setTimeout>;
  return Promise.race([
    request,
    new Promise<never>((_, reject) => { timeout = setTimeout(() => reject(new Error('Startup check timed out')), milliseconds); }),
  ]).finally(() => clearTimeout(timeout));
}

function BootScreen({phase, detail, error, retry}: {phase: number; detail: string; error?: boolean; retry: () => void}) {
  return <div className="boot-screen" role={error ? 'alert' : 'status'} aria-live="polite" aria-busy={!error}>
    <div className="boot-content">
      <div className="boot-mark"><Icon name="sprout"/></div>
      <h1>REI</h1>
      <p className="boot-title">{error ? 'We couldn’t open your workspace' : ['Opening your local workspace…', 'Restoring your preferences…', 'Checking your assistant…'][phase]}</p>
      <p className="boot-detail">{detail}</p>
      {!error && <><ol className="boot-steps" aria-label="Startup progress">{['Workspace', 'Appearance', 'Assistant'].map((name, index) => <li key={name} className={`boot-step ${index < phase ? 'done' : index === phase ? 'active' : ''}`} aria-current={index === phase ? 'step' : undefined}><span className="boot-step-dot" aria-hidden="true"/>{name}</li>)}</ol><div className="boot-line" aria-hidden="true"/></>}
      {error && <button className="boot-button" onClick={retry}>Try again</button>}
      <p className="boot-footnote">A little structure. More room for your day.</p>
    </div>
  </div>;
}

export function Startup({children}: {children: ReactNode}) {
  const [attempt, setAttempt] = useState(0);
  const [phase, setPhase] = useState(0);
  const [detail, setDetail] = useState('Connecting to your saved tasks and calendar.');
  const [error, setError] = useState(false);
  const [ready, setReady] = useState(false);
  const [notice, setNotice] = useState('');

  useEffect(() => {
    window.dispatchEvent(new Event('rei-interface-mounted'));
    // Also works in tests and if the independent fallback script could not load.
    document.getElementById('rei-boot-fallback')?.remove();
    let active = true;
    setReady(false); setError(false); setNotice(''); setPhase(0);
    setDetail('Connecting to your saved tasks and calendar.');
    void (async () => {
      try {
        const status = await within(api.appStatus(), 12000);
        if (!active) return;
        if (status.ipc_version !== 1 || status.schema_version < 1) throw new Error('Unsupported workspace');
      } catch {
        if (active) { setError(true); setDetail('The desktop workspace is not available. Try again, or close and reopen REI. Your saved data has not been reset.'); }
        return;
      }
      setPhase(1); setDetail('Loading your saved colours, text size, and appearance.');
      try {
        const keys = ['theme', 'accent', 'text_size', 'motion'] as const;
        const values = await within(Promise.all(keys.map(key => api.settingsGet(key))), 1800);
        if (!active) return;
        applyTheme(values[0]?.value); applyAccent(values[1]?.value); applyTextSize(values[2]?.value); applyMotion(values[3]?.value);
      } catch { /* Appearance is optional; the normal app can restore it later. */ }
      if (!active) return;
      setPhase(2); setDetail('Reading your saved AI choice. No messages are sent by this check.');
      let ai: AiSettings;
      try {
        ai = await within(api.aiSettings(), 1800);
      } catch {
        if (active) { setNotice('Your planner is ready. Assistant settings could not be checked yet.'); setReady(true); }
        return;
      }
      if (!active) return;
      if (ai.provider === 'local') {
        setDetail(`Checking whether ${ai.local_model || ai.model} is available in Ollama. Your planner can open even if AI is not ready.`);
        try {
          const model = await within(api.modelStatus(ai.local_model || ai.model), 2200);
          if (!active) return;
          if (model.state !== 'ready') setNotice(model.state === 'model_missing' ? 'Your planner is ready. Choose an installed local model in Settings → AI to start summaries.' : 'Your planner is ready. Local AI is not available yet; open Ollama when you want summaries.');
        } catch {
          if (!active) return;
          setNotice('Your planner is ready. Local AI is still unavailable or taking longer to respond.');
        }
      }
      // Hosted AI is not contacted here. Startup does not test keys or run inference.
      if (active) setReady(true);
    })();
    return () => { active = false; };
  }, [attempt]);

  return ready ? <>{notice && <div className="boot-notice" role="status"><span>{notice}</span><a href="#/settings">Open Settings</a><button aria-label="Dismiss startup notice" onClick={() => setNotice('')}>Dismiss</button></div>}{children}</> : <BootScreen phase={phase} detail={detail} error={error} retry={() => setAttempt(n => n + 1)}/>;
}

export class AppErrorBoundary extends Component<{children: ReactNode}, {failed: boolean}> {
  state = {failed: false};
  static getDerivedStateFromError() { return {failed: true}; }
  render() {
    return this.state.failed ? <BootScreen phase={0} error detail="The interface stopped unexpectedly. Reopen REI to try again; your saved workspace has not been reset." retry={() => window.location.reload()}/> : this.props.children;
  }
}
