// Synthetic IPC only; no actual sign-in or OS vault is accessed.
import {cleanup,render,screen,waitFor,within} from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import {DisconnectEmail} from '../../../src/components/DisconnectEmail';
import {setTransport} from '../../../src/ipc';
afterEach(()=>{cleanup();setTransport(null);});
it('asks beside the visible action, cancels without mutation, and disconnects without purging data',async()=>{
 const calls:unknown[]=[];const done=vi.fn(async()=>{});setTransport(async(cmd,args)=>{calls.push({cmd,args});return true;});
 const user=userEvent.setup();render(<DisconnectEmail accountId="synthetic-mail" disabled={false} onDisconnected={done}/>);
 const trigger=screen.getByRole('button',{name:'Disconnect email'});await user.click(trigger);
 let dialog=await screen.findByRole('dialog',{name:'Disconnect email'});expect(dialog).toHaveTextContent('Your messages and tasks stay here');
 await user.click(within(dialog).getByRole('button',{name:'Keep connected'}));expect(calls).toHaveLength(0);expect(trigger).toHaveFocus();
 await user.click(trigger);dialog=await screen.findByRole('dialog',{name:'Disconnect email'});await user.click(within(dialog).getByRole('button',{name:'Confirm disconnect'}));
 await waitFor(()=>expect(done).toHaveBeenCalledOnce());expect(calls).toEqual([{cmd:'connection_disconnect',args:{req:{account_id:'synthetic-mail',purge:false}}}]);
});
it('keeps a failed disconnect visible and prevents a duplicate in-flight request',async()=>{
 let reject:(e:unknown)=>void=()=>{};const done=vi.fn(async()=>{});let count=0;
 setTransport(async()=>{count++;return new Promise((_,no)=>{reject=no;});});
 const user=userEvent.setup();render(<DisconnectEmail accountId="synthetic-mail" disabled={false} onDisconnected={done}/>);
 await user.click(screen.getByRole('button',{name:'Disconnect email'}));await user.click(screen.getByRole('button',{name:'Confirm disconnect'}));
 expect(screen.getByRole('button',{name:'Disconnecting…'})).toBeDisabled();expect(count).toBe(1);
 reject({code:'ProviderBusy',message:'Email check is still finishing'});expect(await screen.findByRole('alert')).toHaveTextContent('Email check is still finishing');expect(done).not.toHaveBeenCalled();expect(screen.getByRole('button',{name:'Confirm disconnect'})).toBeEnabled();
});
