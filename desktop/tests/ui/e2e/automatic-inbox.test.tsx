// Synthetic IPC only; no credentials/accounts/model services are accessed.
import { render,screen,waitFor,within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { AutomaticInbox } from '../../../src/screens/AutomaticInbox';
import { App } from '../../../src/App';
import { setTransport,type ProviderAccount,type Transport } from '../../../src/ipc';
import { InboxFreshness } from '../../../src/components/InboxFreshness';
import { DailyReview } from '../../../src/screens/DailyReview';
import { autoBackend, cand } from '../auto-backend';
afterEach(()=>setTransport(null));
function backend(saved=true) {
  const base=autoBackend();
  const state={ account:saved ? account(true) : null as ProviderAccount|null, loginFails:false, activationFails:false };
  function account(ready:boolean):ProviderAccount { return {account_id:'fixture-hiz',provider:'hiz_imap',kind:'email',display_name:'Synthetic HIZ',state:'consent_needed',evidence:'fixture',settings_saved:true,sign_in_saved:ready,authenticated_at:null,last_read_ok_at:null,last_attempt_at:null,last_success_at:null,last_outcome:null,last_error:null,config:{username:'fixture-student'},sources:[{source_kind:'folder',source_ref:'INBOX',display_name:'INBOX',selected:false},{source_kind:'folder',source_ref:'Junk',display_name:'Junk',selected:false}],stored_messages:0,fetch_running:false}; }
  const transport:Transport=async(cmd,args)=>{
    const req=(args?.req??{}) as Record<string,unknown>;
    if(!['connections_status','hiz_configure','hiz_sign_in','assistant_workflow_set'].includes(cmd)) return base.transport(cmd,args);
    base.calls.push({cmd,args});
    switch(cmd){
      case 'connections_status':return {accounts:state.account?[structuredClone(state.account)]:[],polling:{...base.control.polling},hiz_host:'mail.hiz-saarland.de',hiz_port:993,os_vault_available:true};
      case 'hiz_configure':state.account??=account(false);state.account.config.username=req.username;return state.account.account_id;
      case 'hiz_sign_in':if(state.loginFails)throw {code:'ProviderAuthRequired',message:'Synthetic sign-in rejected'};state.account!.sign_in_saved=true;return ['INBOX','Junk'];
      case 'assistant_workflow_set':
        if(state.activationFails)throw {code:'Validation',message:'Inbox was not found. Choose a mail folder in Advanced settings.'};
        base.settings.enabled=true;base.settings.paused=!req.enabled;base.control.polling.enabled=Boolean(req.enabled);
        state.account!.sources[0].selected=true;
        return {settings:{...base.settings},polling:{...base.control.polling},selected_inbox:true};
    }
  };
  return {...base,state,transport};
}
const writes=(b:ReturnType<typeof backend>)=>b.calls.filter(c=>['hiz_configure','hiz_sign_in','assistant_workflow_set'].includes(c.cmd));
describe('ordinary automatic inbox setup',()=>{
  it('connects with one form, activates the whole pipeline and keeps the sign-in after remount',async()=>{
    const b=backend(false);setTransport(b.transport);const user=userEvent.setup();const view=render(<AutomaticInbox/>);
    await user.type(await screen.findByLabelText('University ID'),'fixture-student');await user.type(screen.getByLabelText('Password'),'SYNTHETIC-pw');
    await user.click(screen.getByRole('button',{name:'Connect and start'}));
    await screen.findByRole('button',{name:'Pause automatic inbox'});
    expect(writes(b).map(c=>c.cmd)).toEqual(['hiz_configure','hiz_sign_in','assistant_workflow_set']);
    expect(b.calls.find(c=>c.cmd==='assistant_workflow_set')?.args).toEqual({req:{enabled:true,account_id:'fixture-hiz'}});
    expect(b.calls.filter(c=>JSON.stringify(c.args ?? {}).includes('SYNTHETIC-pw')).map(c=>c.cmd)).toEqual(['hiz_sign_in']);
    expect(screen.queryByLabelText('Password')).toBeNull();expect(b.calls.some(c=>c.cmd==='connection_fetch_now')).toBe(false);
    expect(b.state.account!.sources.find(s=>s.source_ref==='Junk')!.selected).toBe(false);
    view.unmount();render(<AutomaticInbox/>);await screen.findByRole('button',{name:'Pause automatic inbox'});
    expect(writes(b)).toHaveLength(3);expect(screen.queryByLabelText('Password')).toBeNull();
  });
  it('starts an existing sign-in without logging in again and pauses intake and analysis together',async()=>{
    const b=backend();setTransport(b.transport);const user=userEvent.setup();render(<AutomaticInbox/>);
    await user.click(await screen.findByRole('button',{name:'Start automatic inbox'}));
    expect(await screen.findByRole('button',{name:'Pause automatic inbox'})).toBeInTheDocument();
    await user.click(screen.getByRole('button',{name:'Pause automatic inbox'}));
    await screen.findByRole('button',{name:'Resume automatic inbox'});
    expect(writes(b).map(c=>c.cmd)).toEqual(['assistant_workflow_set','assistant_workflow_set']);
    expect(writes(b).at(-1)?.args).toEqual({req:{enabled:false,account_id:null}});
    expect(b.state.account!.sign_in_saved).toBe(true);expect(b.control.polling.enabled).toBe(false);expect(b.settings.paused).toBe(true);
  });
  it('clears a rejected password, allows fixing the ID and reuses the partially configured account',async()=>{
    const b=backend(false);b.state.loginFails=true;setTransport(b.transport);const user=userEvent.setup();render(<AutomaticInbox/>);
    await user.type(await screen.findByLabelText('University ID'),'wrong-id');await user.type(screen.getByLabelText('Password'),'SYNTHETIC-wrong');
    await user.click(screen.getByRole('button',{name:'Connect and start'}));
    expect(await screen.findByRole('alert')).toHaveTextContent('Synthetic sign-in rejected');expect(screen.getByLabelText('Password')).toHaveValue('');
    expect(b.calls.some(c=>c.cmd==='assistant_workflow_set')).toBe(false);
    b.state.loginFails=false;await user.clear(screen.getByLabelText('University ID'));await user.type(screen.getByLabelText('University ID'),'correct-id');await user.type(screen.getByLabelText('Password'),'SYNTHETIC-pw');
    await user.click(screen.getByRole('button',{name:'Sign in and resume'}));await screen.findByRole('button',{name:'Pause automatic inbox'});
    expect(b.calls.filter(c=>c.cmd==='hiz_configure').map(c=>(c.args?.req as Record<string,unknown>).account_id)).toEqual([null,'fixture-hiz']);
  });
  it('keeps saved credentials if activation fails and reports incomplete setup',async()=>{
    const b=backend(false);b.state.activationFails=true;setTransport(b.transport);const user=userEvent.setup();render(<AutomaticInbox/>);
    await user.type(await screen.findByLabelText('University ID'),'fixture-student');await user.type(screen.getByLabelText('Password'),'SYNTHETIC-pw');await user.click(screen.getByRole('button',{name:'Connect and start'}));
    expect(await screen.findByRole('alert')).toHaveTextContent('Inbox was not found');expect(screen.queryByLabelText('Password')).toBeNull();
    expect(screen.getByRole('button',{name:'Start automatic inbox'})).toBeInTheDocument();expect(b.state.account!.sign_in_saved).toBe(true);expect(b.settings.enabled).toBe(false);
  });
  it('reports local model offline without asking for another email sign-in',async()=>{
    const b=backend();b.settings.enabled=true;b.settings.worker_state='offline';b.control.polling.enabled=true;b.state.account!.sources[0].selected=true;setTransport(b.transport);const user=userEvent.setup();render(<AutomaticInbox/>);
    expect(await screen.findByText(/Open Ollama on this computer/)).toBeInTheDocument();expect(screen.queryByLabelText('Password')).toBeNull();
    await user.click(screen.getByRole('button',{name:'Try again'}));await waitFor(()=>expect(b.calls.some(c=>c.cmd==='auto_queue_retry')).toBe(true));
    expect(b.calls.some(c=>c.cmd==='hiz_sign_in')).toBe(false);expect(screen.queryByText('Microsoft Teams')).toBeNull();
  });
  it('keeps folder/model/timer controls out of ordinary Settings and performs no setup writes on render',async()=>{
    const b=backend();setTransport(b.transport);window.location.hash='#/settings';const user=userEvent.setup();render(<App/>);
    await screen.findByRole('button',{name:'Start automatic inbox'});
    expect(screen.queryByLabelText('Application (client) ID')).toBeNull();expect(screen.queryByLabelText('Installed model')).toBeNull();expect(screen.queryByRole('button',{name:'Save schedule'})).toBeNull();
    expect(writes(b)).toHaveLength(0);expect(b.calls.some(c=>c.cmd==='model_list')).toBe(false);
    await user.click(screen.getByRole('tab',{name:'Advanced'}));await screen.findByText('Diagnostics');
    expect(b.calls.some(c=>c.cmd==='model_list')).toBe(false);
    await user.click(screen.getByText('AI model & time zone'));await screen.findByRole('region',{name:'Local AI'});
    expect(b.calls.some(c=>c.cmd==='model_list')).toBe(true);expect(writes(b)).toHaveLength(0);
  });
});

it('shows important updates immediately and dismisses only the displayed update, keeping originals and task review',async()=>{
  const b=autoBackend();b.settings.enabled=true;b.control.actionable=[cand(1)];b.control.updates=[cand(2,{kind:'update',title:'Lecture moved',due_date:null,date_status:'none'})];
  setTransport(b.transport);const user=userEvent.setup();render(<DailyReview compact/>);
  const now=await screen.findByRole('list',{name:'Important now'});expect(within(now).getByText('Lecture moved')).toBeInTheDocument();
  await user.click(within(now).getByRole('button',{name:'Dismiss update: Lecture moved'}));
  await waitFor(()=>expect(b.control.updates).toHaveLength(0));
  expect(b.calls.find(c=>c.cmd==='candidate_reject')?.args).toEqual({req:{id:'fixture-cand-2',payload_hash:'fixture-hash-2'}});
  expect(b.control.actionable).toHaveLength(1);expect(b.tasks).toHaveLength(0);
  expect(b.calls.some(c=>c.cmd==='connection_disconnect'||c.cmd==='connection_fetch_now')).toBe(false);
  await user.click(screen.getByText(/Review suggestions and updates/));
  expect(await screen.findByRole('button',{name:'Approve 1 selected'})).toBeInTheDocument();
});

it('distinguishes an in-flight check from the first successful read without inventing freshness',async()=>{
  const b=backend();b.control.polling.enabled=true;b.state.account!.fetch_running=true;setTransport(b.transport);
  const view=render(<InboxFreshness/>);expect(await screen.findByLabelText('Email status')).toHaveTextContent('Checking for new email');
  view.unmount();b.state.account!.fetch_running=false;render(<InboxFreshness/>);
  expect(await screen.findByLabelText('Email status')).toHaveTextContent('Waiting for the first successful email check');
  expect(writes(b)).toHaveLength(0);
});

it('finishes missing Inbox scope even when older intake and analysis flags were already on',async()=>{
  const b=backend();b.settings.enabled=true;b.control.polling.enabled=true;setTransport(b.transport);const user=userEvent.setup();render(<AutomaticInbox/>);
  await user.click(await screen.findByRole('button',{name:'Start automatic inbox'}));
  await screen.findByRole('button',{name:'Pause automatic inbox'});
  expect(b.state.account!.sources[0].selected).toBe(true);expect(b.calls.some(c=>c.cmd==='hiz_sign_in')).toBe(false);
});
