import {useId, useRef, useState} from 'react';
import {api, asIpcError} from '../ipc';
import {AnchoredPopover} from './AnchoredPopover';

/** Disconnect is visible beside account status; cached messages and tasks are kept. */
export function DisconnectEmail({accountId, disabled, onDisconnected}: {accountId:string; disabled:boolean; onDisconnected:()=>Promise<void>}) {
  const [open,setOpen]=useState(false);
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState('');
  const trigger=useRef<HTMLButtonElement>(null);
  const running=useRef(false);
  const id=useId();
  function dismiss(){setOpen(false);trigger.current?.focus({preventScroll:true});}
  async function disconnect(){
    if(running.current || disabled)return;
    running.current=true;setBusy(true);setError('');
    try{await api.disconnect(accountId,false);setOpen(false);await onDisconnected();}
    catch(e){setError(asIpcError(e).message);}
    finally{running.current=false;setBusy(false);}
  }
  return <span className="disconnect-email-action"><button ref={trigger} disabled={disabled || busy} aria-haspopup="dialog" aria-expanded={open} aria-controls={open?id:undefined} title={disabled?'Wait for the current email operation to finish':undefined} onClick={()=>{setError('');setOpen(!open);}}>Disconnect email</button>
    {open && <AnchoredPopover id={id} triggerRef={trigger} label="Disconnect email" width={300} onClose={()=>setOpen(false)}><div className="delete-confirmation"><strong>Disconnect email?</strong><p>REI will stop checking this account and remove its saved sign-in from this computer. Your messages and tasks stay here.</p><div className="popover-actions"><button disabled={busy} onClick={dismiss}>Keep connected</button><button className="danger" disabled={busy || disabled} onClick={()=>void disconnect()}>{busy?'Disconnecting…':'Confirm disconnect'}</button></div>{error && <p role="alert">{error}</p>}</div></AnchoredPopover>}
  </span>;
}
