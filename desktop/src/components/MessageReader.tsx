import { useEffect, useRef, type ReactNode } from 'react';
import { createPortal } from 'react-dom';

export function MessageReader({children,onClose,onPrevious,onNext}:{children:ReactNode;onClose:()=>void;onPrevious?:()=>void;onNext?:()=>void}) {
  const ref=useRef<HTMLElement>(null);
  const closeRef=useRef(onClose);closeRef.current=onClose;
  useEffect(()=>{
    const trigger=document.activeElement as HTMLElement|null;
    const shell=document.querySelector('.shell');const inert=shell?.hasAttribute('inert');shell?.setAttribute('inert','');
    const overflow=document.body.style.overflow;document.body.style.overflow='hidden';
    ref.current?.querySelector<HTMLButtonElement>('[aria-label="Close message"]')?.focus();
    const key=(e:KeyboardEvent)=>{
      if(e.key==='Escape'){e.preventDefault();closeRef.current();}
      if(e.key==='Tab'){
        const nodes=[...(ref.current?.querySelectorAll<HTMLElement>('button:not(:disabled),a[href],input:not(:disabled),textarea:not(:disabled),select:not(:disabled),summary,[tabindex="0"]')??[])].filter(el=>!el.closest('details:not([open])') || el.tagName==='SUMMARY');
        const first=nodes[0],last=nodes.at(-1);
        if(e.shiftKey && (document.activeElement===first || !ref.current?.contains(document.activeElement))){e.preventDefault();last?.focus();}
        else if(!e.shiftKey && (document.activeElement===last || !ref.current?.contains(document.activeElement))){e.preventDefault();first?.focus();}
      }
    };
    document.addEventListener('keydown',key);
    return()=>{document.removeEventListener('keydown',key);document.body.style.overflow=overflow;if(!inert)shell?.removeAttribute('inert');if(trigger?.isConnected)trigger.focus();};
  },[]);
  return createPortal(<div className="message-overlay"><section ref={ref} className="message-reader" role="dialog" aria-modal="true" aria-labelledby="reader-heading"><header className="reader-nav"><h2 id="reader-heading">Read message</h2><div><button aria-label="Previous message" disabled={!onPrevious} onClick={onPrevious}>←</button><button aria-label="Next message" disabled={!onNext} onClick={onNext}>→</button><button aria-label="Close message" onClick={onClose}>×</button></div></header>{children}</section></div>,document.body);
}
