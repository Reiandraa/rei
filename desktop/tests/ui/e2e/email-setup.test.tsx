// Synthetic email transports only. No account, credentials, browser or provider contacted.
import { cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { EmailSetup } from '../../../src/screens/EmailSetup';
import { setTransport, type EmailProfile, type ProviderAccount } from '../../../src/ipc';
afterEach(()=>{cleanup();setTransport(null);});
function fixture(kind:EmailProfile['kind']='imap',ready=true){
 const calls:{cmd:string;req:Record<string,unknown>}[]=[];
 const profile:EmailProfile={profile_id:'fixture-profile',kind,email:'owner@example.org',display_name:kind==='google'?'Google':kind==='microsoft'?'Microsoft':'Example Mail',username:'owner@example.org',server:kind==='imap'?'mail.example.org':null,publisher_ready:ready,detail:''};
 let fail=false;
 setTransport(async(cmd,args)=>{const req=args!.req as Record<string,unknown>;calls.push({cmd,req});
  if(cmd==='email_discover')return structuredClone(profile);
  if(cmd==='email_configure')return 'fixture-mail';
  if(cmd==='email_sign_in'){if(fail)throw {code:'ProviderAuthRequired',message:'Synthetic sign-in rejected'};return ['INBOX'];}
  if(cmd==='assistant_workflow_set')return {};
  throw new Error('Unexpected side effect '+cmd);
 });return {calls,profile,reject:()=>{fail=true;},accept:()=>{fail=false;}};
}
it('offers one email/password form and official provider buttons without developer fields or calls on render',()=>{
 const be=fixture();render(<EmailSetup onHiz={vi.fn()}/>);
 expect(screen.getByLabelText('Email')).toBeInTheDocument();expect(screen.getByLabelText('Password')).toHaveAttribute('type','password');
 expect(screen.getByRole('button',{name:'Continue with Google'})).toBeInTheDocument();expect(screen.getByRole('button',{name:'Continue with Microsoft'})).toBeInTheDocument();
 expect(screen.queryByText('Application (client) ID')).toBeNull();expect(be.calls).toHaveLength(0);
});
it('discovers secure email without sending the password, then sends it only to native sign-in before activating',async()=>{
 const be=fixture();const user=userEvent.setup();const connected=vi.fn();render(<EmailSetup onHiz={vi.fn()} onConnected={connected}/>);
 await user.type(screen.getByLabelText('Email'),'owner@example.org');await user.type(screen.getByLabelText('Password'),'SYNTHETIC-password');await user.click(screen.getByRole('button',{name:'Connect email'}));
 await waitFor(()=>expect(connected).toHaveBeenCalled());
 expect(be.calls.map(c=>c.cmd)).toEqual(['email_discover','email_configure','email_sign_in','assistant_workflow_set']);
 expect(be.calls.filter(c=>JSON.stringify(c.req).includes('SYNTHETIC-password')).map(c=>c.cmd)).toEqual(['email_sign_in']);expect(screen.getByLabelText('Password')).toHaveValue('');
});
it.each(['google','microsoft'] as const)('uses %s browser sign-in without sending an entered password or asking for client IDs',async(kind)=>{
 const be=fixture(kind);const user=userEvent.setup();render(<EmailSetup onHiz={vi.fn()}/>);
 await user.type(screen.getByLabelText('Password'),'SYNTHETIC-not-for-browser');
 await user.click(screen.getByRole('button',{name:kind==='google'?'Continue with Google':'Continue with Microsoft'}));
 await waitFor(()=>expect(be.calls.some(c=>c.cmd==='assistant_workflow_set')).toBe(true));
 expect(be.calls.find(c=>c.cmd==='email_discover')?.req.preferred).toBe(kind);
 expect(be.calls.find(c=>c.cmd==='email_sign_in')?.req).toEqual({account_id:'fixture-mail',password:undefined});
 expect(JSON.stringify(be.calls)).not.toContain('SYNTHETIC-not-for-browser');
});
it('does not pretend an unregistered publisher is connected and never starts OAuth or account creation',async()=>{
 const be=fixture('google',false);const user=userEvent.setup();render(<EmailSetup onHiz={vi.fn()}/>);
 await user.click(screen.getByRole('button',{name:'Continue with Google'}));expect(await screen.findByRole('alert')).toHaveTextContent('sign-in is not enabled');
 expect(be.calls.map(c=>c.cmd)).toEqual(['email_discover']);
});
it('retains the partially configured account for retry and never enables fetching after a failed login',async()=>{
 const be=fixture();be.reject();const user=userEvent.setup();render(<EmailSetup onHiz={vi.fn()}/>);
 await user.type(screen.getByLabelText('Email'),'owner@example.org');await user.type(screen.getByLabelText('Password'),'SYNTHETIC-wrong');await user.click(screen.getByRole('button',{name:'Connect email'}));
 expect(await screen.findByRole('alert')).toHaveTextContent('Synthetic sign-in rejected');expect(be.calls.some(c=>c.cmd==='assistant_workflow_set')).toBe(false);
 be.accept();await user.type(screen.getByLabelText('Password'),'SYNTHETIC-correct');await user.click(screen.getByRole('button',{name:'Connect email'}));await waitFor(()=>expect(be.calls.some(c=>c.cmd==='assistant_workflow_set')).toBe(true));expect(be.calls.filter(c=>c.cmd==='email_configure')).toHaveLength(1);
});
it('keeps the existing HIZ ID recovery as an explicit alternative without a fake connection',async()=>{
 const next=vi.fn();render(<EmailSetup onHiz={next}/>);const user=userEvent.setup();
 await user.click(screen.getByText('My university uses a HIZ sign-in ID'));await user.click(screen.getByRole('button',{name:'I already have a HIZ sign-in ID'}));expect(next).toHaveBeenCalledWith('');
});

it('cannot start discovery or sign-in while secure credential storage is unavailable',async()=>{
 const be=fixture();render(<EmailSetup onHiz={vi.fn()} storageReady={false}/>);
 expect(screen.getByRole('button',{name:'Connect email'})).toBeDisabled();
 expect(screen.getByRole('button',{name:'Continue with Google'})).toBeDisabled();
 expect(be.calls).toHaveLength(0);
});

it('can cancel browser sign-in again for an existing account without disconnecting it',async()=>{
 const calls:string[]=[];let rejectLogin:(e:unknown)=>void=()=>{};
 setTransport(async(cmd)=>{calls.push(cmd);if(cmd==='email_sign_in')return new Promise((_,reject)=>{rejectLogin=reject;});if(cmd==='connection_fetch_cancel'){rejectLogin({code:'Cancelled',message:'Sign-in cancelled'});return true;}throw new Error('Unexpected side effect '+cmd);});
 const account:ProviderAccount={account_id:'existing-google',provider:'google_mail',kind:'email',display_name:'Google',state:'expired',evidence:'fixture',settings_saved:true,sign_in_saved:true,authenticated_at:null,last_read_ok_at:null,last_attempt_at:null,last_success_at:null,last_outcome:null,last_error:null,config:{email:'owner@example.org'},sources:[],stored_messages:0,fetch_running:false};
 const user=userEvent.setup();render(<EmailSetup account={account} onHiz={vi.fn()}/>);
 await user.click(screen.getByRole('button',{name:'Sign in again in browser'}));
 await user.click(await screen.findByRole('button',{name:'Cancel connection'}));
 expect(await screen.findByRole('alert')).toHaveTextContent('Sign-in cancelled');
 expect(screen.getByRole('button',{name:'Sign in again in browser'})).toBeEnabled();
 expect(calls).toEqual(['email_sign_in','connection_fetch_cancel']);
});
