import { useId, useRef, useState } from 'react';
import { asIpcError } from '../ipc';
import { AnchoredPopover } from './AnchoredPopover';

export function DeleteAction({title,kind='task',disabled=false,onConfirm}:{title:string;kind?:'task'|'event'|'routine'|'preference'|'API key';disabled?:boolean;onConfirm:()=>Promise<boolean|void>}) {
  const [open,setOpen]=useState(false);
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string|null>(null);
  const trigger=useRef<HTMLButtonElement>(null);
  const id=useId();
  const dismiss=()=>{setOpen(false);trigger.current?.focus({preventScroll:true});};
  async function confirm(){
    if(busy)return;setBusy(true);setError(null);
    try{if(await onConfirm()!==false){setOpen(false);if(trigger.current?.isConnected)trigger.current.focus({preventScroll:true});else document.querySelector<HTMLElement>('main h1')?.focus({preventScroll:true});}}
    catch(e){setError(asIpcError(e).message);}finally{setBusy(false);}
  }
  return <span className="delete-action"><button ref={trigger} className="task-delete-button" disabled={disabled || busy} aria-label={`Delete: ${title}`} aria-haspopup="dialog" aria-expanded={open} aria-controls={open?id:undefined} onClick={()=>{setError(null);setOpen(!open);}}>Delete</button>
    {open && <AnchoredPopover id={id} triggerRef={trigger} label={`Delete ${kind}: ${title}`} onClose={()=>setOpen(false)}><div className="delete-confirmation"><strong>Delete {kind}?</strong><p>{title}</p>{kind==='routine' && <p className="muted">This removes the repeating routine and its meetings.</p>}<div className="popover-actions"><button onClick={dismiss}>Keep {kind==='routine'?'task':kind}</button><button className="danger" disabled={busy} aria-label={`Confirm delete: ${title}`} onClick={()=>void confirm()}>{busy?'Deleting…':'Delete'}</button></div>{error && <p role="alert">{error}</p>}</div></AnchoredPopover>}
  </span>;
}
