import { useId, useRef, useState } from 'react';
import { AnchoredPopover } from './AnchoredPopover';
import { asIpcError, type AgendaItem } from '../ipc';

/** Completion is a check; skipping is a distinct, less frequent action in the menu. */
export function OccurrenceControls({item:i,onEdit,onOpenSchedule}:{item:AgendaItem;onEdit:(item:AgendaItem,action:'done'|'skip'|'reset')=>Promise<unknown>;onOpenSchedule?:()=>void}) {
  const [open,setOpen]=useState(false);
  const trigger=useRef<HTMLButtonElement>(null);
  const id=useId();
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string|null>(null);
  const run=async(action:'done'|'skip'|'reset')=>{
    if(busy)return;setBusy(true);setError(null);
    try{await onEdit(i,action);}catch(e){setError(asIpcError(e).message);}finally{setBusy(false);}
  };
  return <div className="occurrence-controls">
    {i.state==='Skipped'?<><small>Skipped</small><button disabled={busy} aria-label={`Undo: ${i.title}`} onClick={()=>void run('reset')}>Undo skip</button></>:<>
      <label className="occurrence-completion"><input type="checkbox" disabled={busy} checked={i.state==='Done'} aria-label={i.state==='Done'?`Undo: ${i.title}`:`Mark done this time: ${i.title}`} onChange={()=>void run(i.state==='Done'?'reset':'done')} /><span>{i.state==='Done'?'Completed':'Complete'}</span></label>
      {i.state!=='Done' && <span className="occurrence-menu"><button ref={trigger} className="occurrence-menu-trigger" disabled={busy} aria-label={`More options: ${i.title}`} aria-haspopup="dialog" aria-expanded={open} aria-controls={open?id:undefined} onClick={()=>setOpen(!open)}>⋯</button>
        {open && <AnchoredPopover id={id} triggerRef={trigger} label={`Schedule options: ${i.title}`} width={220} onClose={()=>setOpen(false)}>{onOpenSchedule && <button className="popover-option" disabled={busy} aria-label={`Edit schedule: ${i.title}`} onClick={()=>{setOpen(false);trigger.current?.focus({preventScroll:true});onOpenSchedule();}}>Edit schedule</button>}<button className="popover-option" disabled={busy} aria-label={`Skip this time: ${i.title}`} onClick={()=>{setOpen(false);trigger.current?.focus({preventScroll:true});void run('skip');}}>Skip this occurrence</button></AnchoredPopover>}
      </span>}
    </>}
    {error && <span role="alert">{error}</span>}
  </div>;
}
