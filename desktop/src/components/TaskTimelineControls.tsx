import { useId, useLayoutEffect, useRef, useState } from 'react';
import { asIpcError, type Task } from '../ipc';
import { AnchoredPopover } from './AnchoredPopover';
import type { EditorState } from './TaskEditor';

export type TimelineTaskAction = 'complete' | 'reopen' | 'unplan' | 'delete';

/** The same controls as a meeting, with actions that operate on a one-time task. */
export function TaskTimelineControls({task,day,edit,onAction}:{
  task:Task;day:string;edit:(editor:EditorState)=>void;
  onAction:(task:Task,action:TimelineTaskAction)=>Promise<boolean|void>;
}) {
  const [open,setOpen]=useState(false);
  const [confirming,setConfirming]=useState(false);
  const [busy,setBusy]=useState(false);
  const [error,setError]=useState<string|null>(null);
  const trigger=useRef<HTMLButtonElement>(null);
  const keep=useRef<HTMLButtonElement>(null);
  const id=useId();
  useLayoutEffect(()=>{if(confirming)keep.current?.focus({preventScroll:true});},[confirming]);
  const close=()=>{setOpen(false);setConfirming(false);};
  const dismiss=()=>{close();trigger.current?.focus({preventScroll:true});};
  async function run(action:TimelineTaskAction) {
    if(busy)return;setBusy(true);setError(null);
    try {
      if(await onAction(task,action)!==false){
        if(open)dismiss();
        if(action==='delete' && !trigger.current?.isConnected)document.querySelector<HTMLElement>('main h1')?.focus({preventScroll:true});
      }
    } catch(e){setError(asIpcError(e).message);}finally{setBusy(false);}
  }
  const openEditor=(mode:'edit'|'plan')=>{dismiss();edit({mode,task});};
  return <div className="occurrence-controls">
    <label className="occurrence-completion"><input type="checkbox" disabled={busy} checked={task.status==='done'} aria-label={task.status==='done'?`Reopen: ${task.title}`:`Mark done: ${task.title}`} onChange={()=>void run(task.status==='done'?'reopen':'complete')}/><span>{task.status==='done'?'Completed':'Complete'}</span></label>
    {task.status!=='done' && <span className="occurrence-menu"><button ref={trigger} className="occurrence-menu-trigger" disabled={busy} aria-label={`More options: ${task.title}`} aria-haspopup="dialog" aria-expanded={open} aria-controls={open?id:undefined} onClick={()=>{setError(null);setConfirming(false);setOpen(!open);}}>⋯</button>
      {open && <AnchoredPopover id={id} triggerRef={trigger} label={`${confirming?'Delete task':'Task options'}: ${task.title}`} width={confirming?260:220} onClose={close}>
        {confirming?<div className="delete-confirmation"><strong>Delete task?</strong><p>{task.title}</p><div className="popover-actions"><button ref={keep} disabled={busy} onClick={dismiss}>Keep task</button><button className="danger" disabled={busy} aria-label={`Confirm delete: ${task.title}`} onClick={()=>void run('delete')}>{busy?'Deleting…':'Delete'}</button></div></div>:<>
          <button className="popover-option" disabled={busy} onClick={()=>openEditor('edit')}>Edit task</button>
          <button className="popover-option" disabled={busy} onClick={()=>openEditor('plan')}>{task.plan_date?'Move plan':'Plan task'}</button>
          {task.plan_date===day && <button className="popover-option" disabled={busy} onClick={()=>void run('unplan')}>Remove from Today</button>}
          <button className="popover-option danger" disabled={busy} onClick={()=>setConfirming(true)}>Delete task</button>
        </>}
        {error && <p role="alert">{error}</p>}
      </AnchoredPopover>}
    </span>}
    {error && !open && <span role="alert">{error}</span>}
  </div>;
}
