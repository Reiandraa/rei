// Synthetic setup state only: no installed-app launch, model download or API call.
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { LocalAiSetup } from '../../../src/screens/LocalAiSetup';
import { setTransport, type LocalSetup } from '../../../src/ipc';

afterEach(() => { cleanup(); setTransport(null); });
function fixture(installed = false) {
  const calls: {action: string; model?: string}[] = [];
  const state: LocalSetup = {platform:'macos',installed,memory_gb:16,recommended_model:'qwen3.5:4b',download:{state:'idle',model:'',detail:'',completed:0,total:0}};
  setTransport(async (cmd,args) => {
    if(cmd !== 'local_ai_setup') throw new Error('Unexpected setup side effect');
    const req = args!.req as {action:string;model?:string}; calls.push(req);
    if(req.action === 'download') state.download={state:'downloading',model:req.model!,detail:'Preparing model files…',completed:0,total:0};
    if(req.action === 'cancel') state.download.state='cancelling';
    return structuredClone(state);
  });
  return {state,calls};
}
const noServer={state:'not_running',models:[]};
const noModels={state:'ready',models:[]};
const refresh=()=>Promise.resolve();

it('explains a missing Mac installation, opens the installer only after a click, and rechecks on return', async () => {
  const be=fixture();const user=userEvent.setup();const onRefresh=vi.fn(refresh);
  render(<LocalAiSetup list={noServer} current="qwen3.5:9b" busy={false} onRefresh={onRefresh} onSave={vi.fn()}/>);
  await screen.findByText('Install Ollama to use AI on this computer.');
  expect(be.calls.every(c=>c.action==='inspect')).toBe(true);
  await user.click(screen.getByRole('button',{name:'Install Ollama'}));
  expect(await screen.findByText(/official download page is open/)).toBeInTheDocument();
  expect(screen.getByText(/drag Ollama into Applications/)).toBeInTheDocument();
  fireEvent.focus(window);await waitFor(()=>expect(onRefresh).toHaveBeenCalled());
  expect(be.calls.filter(c=>c.action==='installer')).toHaveLength(1);
  expect(be.calls.some(c=>c.action==='download')).toBe(false);
});

it('offers opening an installed app instead of reinstalling it', async () => {
  const be=fixture(true);const user=userEvent.setup();
  render(<LocalAiSetup list={noServer} current="qwen3.5:9b" busy={false} onRefresh={refresh} onSave={vi.fn()}/>);
  const open=await screen.findByRole('button',{name:'Open Ollama'});
  expect(screen.queryByRole('button',{name:'Install Ollama'})).toBeNull();
  await user.click(open);expect(be.calls.some(c=>c.action==='open')).toBe(true);
});

it('requires an explicit download, uses a RAM starting point, reports file progress and can cancel', async () => {
  const be=fixture(true);const user=userEvent.setup();const save=vi.fn();
  render(<LocalAiSetup list={noModels} current="qwen3.5:9b" busy={false} onRefresh={refresh} onSave={save}/>);
  await waitFor(()=>expect(screen.getByLabelText('Model to download')).toHaveValue('qwen3.5:4b'));
  expect(screen.getByText(/16 GB RAM detected/)).toHaveTextContent('not a GPU benchmark');
  expect(be.calls.every(c=>c.action==='inspect')).toBe(true);
  await user.click(screen.getByRole('button',{name:'Download model'}));
  expect(be.calls.find(c=>c.action==='download')?.model).toBe('qwen3.5:4b');
  expect(screen.queryByRole('progressbar')).toBeNull();
  be.state.download={state:'downloading',model:'qwen3.5:4b',detail:'Downloading a model file…',completed:40,total:100};
  fireEvent.focus(window);
  expect(await screen.findByRole('progressbar',{name:'Current model file download'})).toHaveAttribute('value','40');
  await user.click(screen.getByRole('button',{name:'Cancel download'}));
  expect(await screen.findByRole('button',{name:'Stopping…'})).toBeDisabled();
  expect(save).not.toHaveBeenCalled();
});

it('keeps the configured model until the installed download is explicitly selected', async () => {
  const be=fixture(true);const save=vi.fn();const user=userEvent.setup();
  be.state.download={state:'complete',model:'qwen3.5:4b',detail:'Model installed. Choose it below to use it in REI.',completed:100,total:100};
  render(<LocalAiSetup list={{state:'ready',models:['qwen3.5:4b']}} current="qwen3.5:9b" busy={false} onRefresh={refresh} onSave={save}/>);
  expect(await screen.findByRole('button',{name:'Use qwen3.5:4b'})).toBeInTheDocument();
  expect(screen.queryByText('Ready · qwen3.5:9b')).toBeNull();
  expect(save).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button',{name:'Use qwen3.5:4b'}));
  expect(save).toHaveBeenCalledWith('qwen3.5:4b');
});

it('shows failed checks as a connection problem without offering a model download or claiming readiness', async () => {
  fixture(true);
  render(<LocalAiSetup list={{state:'error',models:[]}} current="qwen3.5:9b" busy={false} onRefresh={refresh} onSave={vi.fn()}/>);
  expect(await screen.findByText('Ollama could not be checked. Open it and try again.')).toBeInTheDocument();
  expect(screen.queryByRole('button',{name:'Download model'})).toBeNull();
});
