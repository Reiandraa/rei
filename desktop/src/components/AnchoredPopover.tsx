import { useId, useLayoutEffect, useRef, useState, type ReactNode, type RefObject } from 'react';
import { createPortal } from 'react-dom';

/** A small non-modal surface, outside scrolling cards so it cannot resize or be clipped by them. */
export function AnchoredPopover({triggerRef,label,width=260,onClose,children,id}:{triggerRef:RefObject<HTMLElement>;label:string;width?:number;onClose:()=>void;children:ReactNode;id:string}) {
  const panel=useRef<HTMLDivElement>(null);
  const close=useRef(onClose);close.current=onClose;
  const [position,setPosition]=useState<{left:number;top:number;width:number}|null>(null);
  const titleId=useId();
  const ready=position!==null;
  useLayoutEffect(()=>{if(ready)panel.current?.querySelector<HTMLButtonElement>('button:not(:disabled)')?.focus({preventScroll:true});},[ready]);
  useLayoutEffect(()=>{
    const measure=()=>{
      const anchor=triggerRef.current,box=panel.current;if(!anchor || !anchor.isConnected || !box){close.current();return;}
      const rect=anchor.getBoundingClientRect();const vw=window.innerWidth,vh=window.innerHeight;
      if(rect.width && rect.height && (rect.bottom<0 || rect.top>vh || rect.right<0 || rect.left>vw)){close.current();return;}
      const w=Math.min(width,Math.max(1,vw-16));const h=box.getBoundingClientRect().height;
      const below=rect.bottom+8;const top=below+h<=vh-8?below:Math.max(8,rect.top-8-h);
      setPosition({left:Math.max(8,Math.min(rect.right-w,vw-w-8)),top:Math.min(top,Math.max(8,vh-h-8)),width:w});
    };
    measure();
    const restore=()=>{triggerRef.current?.focus({preventScroll:true});close.current();};
    const outside=(event:Event)=>{const target=event.target;if(target instanceof Node && !panel.current?.contains(target) && !triggerRef.current?.contains(target))close.current();};
    const key=(event:KeyboardEvent)=>{if(event.key==='Escape'){event.preventDefault();event.stopPropagation();restore();}};
    document.addEventListener('pointerdown',outside,true);document.addEventListener('focusin',outside,true);document.addEventListener('keydown',key,true);
    window.addEventListener('resize',measure);window.addEventListener('scroll',measure,true);
    const observer=typeof ResizeObserver==='undefined'?null:new ResizeObserver(measure);if(panel.current)observer?.observe(panel.current);
    return()=>{document.removeEventListener('pointerdown',outside,true);document.removeEventListener('focusin',outside,true);document.removeEventListener('keydown',key,true);window.removeEventListener('resize',measure);window.removeEventListener('scroll',measure,true);observer?.disconnect();};
  },[triggerRef,width]);
  return createPortal(<div ref={panel} id={id} className="anchored-popover" role="dialog" aria-labelledby={titleId} style={{left:position?.left??0,top:position?.top??0,width:position?.width??Math.min(width,window.innerWidth-16),visibility:position?'visible':'hidden'}}><span id={titleId} className="sr-only">{label}</span>{children}</div>,document.body);
}
