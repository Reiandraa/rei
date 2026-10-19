// Synthetic IPC only: no real API, credentials, model billing, OS vault or profile.
import { cleanup,render,screen,within,waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../../../src/App';
import { AiSettings } from '../../../src/screens/AiSettings';
import { setTransport,type AiRequest,type AiSettings as Settings } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';
afterEach(()=>{cleanup();setTransport(null);});
function backend(){
  const base=syntheticBackend();
  const state:Settings={revision:0,provider:'local',model:'synthetic-local',local_model:'synthetic-local',openai_saved:false,anthropic_saved:false,daily_limit:50,used_today:0,available_models:[]};
  const calls:AiRequest[]=[];let failure=false;
  setTransport(async(cmd,args)=>{
    if(cmd!=='ai_settings')return base.transport(cmd,args);
    const req=args!.req as AiRequest;calls.push(structuredClone(req));
    if(req.action==='get')return structuredClone(state);
    if(req.expected_revision!==state.revision)throw {code:'Conflict',message:'Reload AI settings.'};
    if(req.action==='save_key'){state[req.provider==='openai'?'openai_saved':'anthropic_saved']=true;state.revision++;}
    if(req.action==='test'){if(failure)throw {code:'ProviderAuthRequired',message:'API access denied.'};return {...structuredClone(state),available_models:['synthetic-text-model']};}
    if(req.action==='use_api'){expect(req.consent).toBe(true);state.provider=req.provider!;state.model=req.model!;state.daily_limit=req.daily_limit!;state.revision++;}
    if(req.action==='use_local'){state.provider='local';state.model=state.local_model;state.revision++;}
    if(req.action==='delete_key'){expect(req.consent).toBe(true);state[req.provider==='openai'?'openai_saved':'anthropic_saved']=false;state.provider='local';state.model=state.local_model;state.revision++;}
    return structuredClone(state);
  });return {calls,state,base,deny:()=>{failure=true;}};
}

it('makes AI discoverable in Settings and never checks a remote account just by opening it',async()=>{
  const be=backend();window.location.hash='#/settings?tab=ai';render(<App/>);
  expect(await screen.findByRole('heading',{name:'Your AI, your choice'})).toBeInTheDocument();
  await screen.findByText(/Active · Local Ollama/);
  expect(screen.getByRole('tab',{name:'AI'})).toHaveAttribute('aria-selected','true');
  expect(be.calls.every(c=>c.action==='get')).toBe(true);
});

it('checks a write-only key before explicit consent enables the chosen API and model',async()=>{
  const be=backend();const user=userEvent.setup();render(<AiSettings localControls={<p>Local controls</p>}/>);
  await screen.findByText('Local controls');
  await user.click(screen.getByRole('button',{name:/OpenAI Your API/}));
  const key=screen.getByLabelText('API key');expect(key).toHaveAttribute('type','password');
  await user.type(key,'fixture-api-key-NOT-REAL');await user.click(screen.getByRole('button',{name:'Check API key'}));
  expect(await screen.findByText(/API key accepted/)).toBeInTheDocument();
  expect(key).toHaveValue('');
  expect(be.calls.filter(c=>c.action!=='get').map(c=>c.action)).toEqual(['save_key','test']);
  await user.type(screen.getByLabelText('API model'),'synthetic-text-model');
  expect(screen.getByRole('button',{name:'Enable OpenAI API'})).toBeDisabled();
  await user.click(screen.getByRole('checkbox',{name:/I allow REI to send/}));
  await user.click(screen.getByRole('button',{name:'Enable OpenAI API'}));
  await screen.findByText(/API is active for analysis/);
  expect(be.calls.at(-1)).toEqual({action:'use_api',provider:'openai',model:'synthetic-text-model',consent:true,daily_limit:50,expected_revision:1});
  expect(screen.queryByText('fixture-api-key-NOT-REAL')).toBeNull();
});

it('keeps failed connection checks off and clears entered keys and consent when changing provider',async()=>{
  const be=backend();be.deny();const user=userEvent.setup();render(<AiSettings localControls={<p>Local controls</p>}/>);await screen.findByText('Local controls');
  await user.click(screen.getByRole('button',{name:/Claude \/ Anthropic Your API/}));
  await user.type(screen.getByLabelText('API key'),'fixture-api-key-NOT-REAL');await user.click(screen.getByRole('button',{name:'Check API key'}));
  expect(await screen.findByRole('alert')).toHaveTextContent('API access denied.');expect(screen.getByLabelText('API key')).toHaveValue('');
  expect(be.state.provider).toBe('local');expect(be.calls.some(c=>c.action==='use_api')).toBe(false);
  await user.click(screen.getByRole('checkbox',{name:/I allow REI to send/}));await user.type(screen.getByLabelText('API key'),'transient-key');
  await user.click(screen.getByRole('button',{name:/OpenAI Your API/}));
  expect(screen.getByLabelText('API key')).toHaveValue('');expect(screen.getByRole('checkbox',{name:/I allow REI to send/})).not.toBeChecked();
});

it('uses a saved API without exposing its key and requires nearby confirmation to remove it',async()=>{
  const be=backend();be.state.provider='anthropic';be.state.model='synthetic-text-model';be.state.anthropic_saved=true;be.state.used_today=4;
  const user=userEvent.setup();render(<AiSettings localControls={<p>Local controls</p>}/>);
  expect(await screen.findByText(/Active · Claude API/)).toBeInTheDocument();expect(screen.getByLabelText('API key')).toHaveValue('');
  await user.click(screen.getByRole('button',{name:'Delete: Claude / Anthropic API key'}));
  expect(be.calls.some(c=>c.action==='delete_key')).toBe(false);
  const dialog=screen.getByRole('dialog');await user.click(within(dialog).getByRole('button',{name:'Confirm delete: Claude / Anthropic API key'}));
  await waitFor(()=>expect(be.state.provider).toBe('local'));expect(be.calls.some(c=>c.action==='delete_key'&&c.consent)).toBe(true);
  expect(await screen.findByText(/API key removed/)).toBeInTheDocument();
});
