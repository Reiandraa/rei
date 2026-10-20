// Synthetic local IPC only. These assertions cover behavior, not native layout or model quality.
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../../../src/App';
import { ConnectionsPanel } from '../../../src/screens/Connections';
import { TimeZoneSettings } from '../../../src/screens/SettingsScreen';
import { DailyReview } from '../../../src/screens/DailyReview';
import { setTransport, type ProviderAccount } from '../../../src/ipc';
import { autoBackend, cand } from '../auto-backend';

function backend(){
  const base=autoBackend();
  const account:ProviderAccount={account_id:'fixture-hiz',provider:'hiz_imap',kind:'email',display_name:'Synthetic HIZ',state:'connected',evidence:'fixture',settings_saved:true,sign_in_saved:true,authenticated_at:null,last_read_ok_at:'2026-10-19T10:00:00Z',last_attempt_at:null,last_success_at:null,last_outcome:null,last_error:null,config:{username:'fixture-student'},sources:[{source_kind:'folder',source_ref:'INBOX',display_name:'INBOX',selected:true},{source_kind:'folder',source_ref:'Junk',display_name:'Junk',selected:false}],stored_messages:13,fetch_running:false};
  const transport:typeof base.transport=async(cmd,args)=>{
    if(cmd==='connections_status')return {accounts:[structuredClone(account)],polling:{...base.control.polling},hiz_host:'mail.hiz-saarland.de',hiz_port:993,os_vault_available:true};
    if(cmd==='connection_select_sources'){base.calls.push({cmd,args});for(const folder of account.sources)folder.selected=(args!.req as {selected:{source_ref:string}[]}).selected.some(s=>s.source_ref===folder.source_ref);return 1;}
    if(cmd==='model_list')return {state:'ready',models:['qwen3.5:9b']};
    return base.transport(cmd,args);
  };
  return {...base,account,transport};
}
afterEach(()=>{cleanup();setTransport(null);for(const attr of ['data-theme','data-accent','data-text-size','data-motion'])document.documentElement.removeAttribute(attr);});

it('offers only Light/Dark, restores the retired Black value as Dark, and never rewrites preferences on opening',async()=>{
  const be=backend();be.store.set('theme',{value:'black',revision:4});setTransport(be.transport);window.location.hash='#/settings?tab=appearance';render(<App/>);
  const select=await screen.findByRole('combobox',{name:'Appearance'});expect(select).toHaveValue('dark');expect(document.documentElement).toHaveAttribute('data-theme','dark');expect(within(select).getAllByRole('option').map(o=>o.textContent)).toEqual(['Light','Dark']);
  expect(within(screen.getByRole('group',{name:'Accent colour'})).getAllByRole('button')).toHaveLength(8);
  expect(be.calls.some(c=>c.cmd==='settings_set')).toBe(false);expect(be.store.get('theme')).toEqual({value:'black',revision:4});
});

it('persists Ocean, Sunrise, text size and reduced animations, restoring them on another page',async()=>{
  const be=backend();setTransport(be.transport);window.location.hash='#/settings?tab=appearance';const user=userEvent.setup();const view=render(<App/>);
  await user.click(await screen.findByRole('button',{name:'Ocean'}));expect(document.documentElement).toHaveAttribute('data-accent','ocean');
  await user.click(screen.getByRole('button',{name:'Sunrise'}));await user.selectOptions(screen.getByLabelText('Text size'),'large');await user.click(screen.getByRole('checkbox',{name:'Reduce animations'}));
  expect(be.store.get('accent')).toEqual({value:'sunrise',revision:2});expect(be.store.get('text_size')).toEqual({value:'large',revision:1});expect(be.store.get('motion')).toEqual({value:'reduced',revision:1});
  view.unmount();for(const a of ['data-accent','data-text-size','data-motion'])document.documentElement.removeAttribute(a);window.location.hash='#/calendar';render(<App/>);
  await waitFor(()=>expect(document.documentElement).toHaveAttribute('data-text-size','large'));expect(document.documentElement).toHaveAttribute('data-accent','sunrise');expect(document.documentElement).toHaveAttribute('data-motion','reduced');
});

it('keeps the accepted text size after a refused save',async()=>{
  const be=backend();be.store.set('text_size',{value:'standard',revision:2});setTransport(async(cmd,args)=>{if(cmd==='settings_set')throw {code:'Internal',message:'Synthetic storage refusal'};return be.transport(cmd,args);});window.location.hash='#/settings?tab=appearance';const user=userEvent.setup();render(<App/>);
  await user.selectOptions(await screen.findByLabelText('Text size'),'large');expect(await within(screen.getByRole('region',{name:'Your settings'})).findByRole('alert')).toHaveTextContent('Synthetic storage refusal');expect(screen.getByLabelText('Text size')).toHaveValue('standard');expect(document.documentElement).toHaveAttribute('data-text-size','standard');
});

it('unfolds only the requested advanced section, without duplicate pause controls or sign-in prompts',async()=>{
  const be=backend();be.settings.enabled=true;be.control.polling.enabled=true;setTransport(be.transport);window.location.hash='#/settings?tab=appearance';const user=userEvent.setup();render(<App/>);
  await user.click(await screen.findByRole('tab',{name:'Connections'}));expect(screen.queryByLabelText('HIZ password')).toBeNull();expect(screen.queryByLabelText('Installed model')).toBeNull();expect(screen.queryByRole('button',{name:'Turn off'})).toBeNull();
  await user.click(screen.getByText('Email preferences'));expect(await screen.findByRole('region',{name:'Mailbox preferences'})).toBeInTheDocument();expect(screen.getByLabelText('INBOX')).toBeChecked();expect(screen.getByLabelText('Junk')).not.toBeChecked();
  expect(screen.getByText('Reconnect email').closest('details')).not.toHaveAttribute('open');expect(within(screen.getByRole('region',{name:'Mailbox preferences'})).getByText('Disconnect email').closest('details')).not.toHaveAttribute('open');expect(screen.queryByRole('button',{name:'Save schedule'})).toBeNull();
  await user.click(screen.getByLabelText('Junk'));await user.click(screen.getByRole('button',{name:'Save folders'}));await waitFor(()=>expect(be.account.sources[1].selected).toBe(true));expect(be.account.sign_in_saved).toBe(true);
  await user.click(screen.getByRole('tab',{name:'Advanced'}));await user.click(screen.getByText('Time zone',{exact:true}));expect(await screen.findByRole('textbox',{name:'Time zone'})).toHaveValue('Europe/Berlin');expect(screen.queryByLabelText('Installed model')).toBeNull();expect(be.calls.some(c=>['hiz_sign_in','hiz_configure','assistant_workflow_set'].includes(c.cmd))).toBe(false);
});

it('changes the email interval without resuming a pipeline paused elsewhere',async()=>{
  const be=backend();be.control.polling.enabled=true;setTransport(be.transport);const user=userEvent.setup();render(<ConnectionsPanel compact showTeams={false}/>);
  const input=await screen.findByLabelText('Minutes between checks');fireEvent.change(input,{target:{value:'45'}});be.control.polling.enabled=false;
  await user.click(screen.getByRole('button',{name:'Save interval'}));await waitFor(()=>expect(be.calls.some(c=>c.cmd==='polling_set')).toBe(true));expect(be.calls.find(c=>c.cmd==='polling_set')!.args).toEqual({req:{enabled:false,interval_minutes:45}});expect(be.account.sign_in_saved).toBe(true);
});

it('recovers stopped analyses without changing model, opt-in or email credentials',async()=>{
  const be=backend();be.settings.enabled=true;be.control.queue.failed=2;setTransport(be.transport);window.location.hash='#/settings?tab=appearance';const user=userEvent.setup();render(<App/>);
  await user.click(await screen.findByRole('tab',{name:'Advanced'}));await user.click(screen.getByText('Diagnostics',{exact:true}));await user.click(screen.getByText('Message recovery'));await user.click(await screen.findByRole('button',{name:'Retry stopped messages'}));
  await waitFor(()=>expect(be.control.queue.queued).toBe(2));expect(be.settings.model).toBe('qwen3.5:9b');expect(be.settings.enabled).toBe(true);expect(be.calls.some(c=>['auto_settings_set','hiz_sign_in','assistant_workflow_set'].includes(c.cmd))).toBe(false);
});

it('keeps the daily review as one area with one Settings link and retains exact approval behavior',async()=>{
  const be=backend();be.settings.enabled=true;be.control.actionable=[cand(1)];setTransport(be.transport);const user=userEvent.setup();render(<DailyReview compact/>);
  const review=await screen.findByRole('region',{name:'Daily review'});expect(within(review).getAllByRole('link',{name:/settings/i})).toHaveLength(1);expect(within(review).getByLabelText('Review counts')).toHaveTextContent('1 suggestion');
  await user.click(within(review).getByText(/Review suggestions and updates/));await user.click(await within(review).findByRole('button',{name:'Approve 1 selected'}));await waitFor(()=>expect(be.tasks).toHaveLength(1));expect(be.control.batches[0]).toEqual([{id:'fixture-cand-1',payload_hash:'fixture-hash-1'}]);
});


it('saves the time zone without changing the current AI provider, pause state or installed model',async()=>{
  const be=backend();setTransport(be.transport);const user=userEvent.setup();render(<TimeZoneSettings/>);
  const field=await screen.findByRole('textbox',{name:'Time zone'});
  be.settings.model='api_openai/synthetic-model';be.settings.enabled=false;be.settings.paused=true;
  await user.clear(field);await user.type(field,'Asia/Jakarta');await user.click(screen.getByRole('button',{name:'Save time zone'}));
  await waitFor(()=>expect(be.calls.some(c=>c.cmd==='auto_settings_set')).toBe(true));
  expect(be.calls.find(c=>c.cmd==='auto_settings_set')?.args).toEqual({req:{enabled:false,paused:true,model:'api_openai/synthetic-model',date_zone:'Asia/Jakarta'}});
  expect(be.calls.some(c=>['model_list','local_ai_setup','ai_settings','email_sign_in'].includes(c.cmd))).toBe(false);
});
