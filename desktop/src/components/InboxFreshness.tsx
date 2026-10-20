import { useEffect, useState } from 'react';
import { api, type ConnectionsStatus } from '../ipc';
/** Read-only stored intake status: being up to date locally is not proof the mailbox was fetched. */
export function InboxFreshness({showLink=true}:{showLink?:boolean}) {
  const [status,setStatus]=useState<ConnectionsStatus|null>(null);
  const [failed,setFailed]=useState(false);
  useEffect(()=>{
    let alive=true;
    const load=async()=>{try{const s=await api.connectionsStatus();if(alive){setStatus(s);setFailed(false);}}catch{if(alive)setFailed(true);}};
    void load();const t=setInterval(()=>void load(),15000);
    return()=>{alive=false;clearInterval(t);};
  },[]);
  if(failed)return <p className="muted" aria-label="Email status">Email status is unavailable. {showLink && <a href="#/settings">Check connection</a>}</p>;
  if(!status)return null;
  const accounts=status.accounts.filter(a=>a.kind==='email'&&a.sign_in_saved);
  let text='No saved email sign-in. Stored messages can still be reviewed.';
  if(accounts.some(a=>['expired','permission_denied'].includes(a.state)))text='Email needs a new sign-in. Stored messages are kept.';
  else if(accounts.some(a=>a.fetch_running))text='Checking for new email…';
  else if(accounts.length&&!status.polling.enabled)text='Automatic email checking is paused.';
  else if(accounts.length){
    const last=accounts.map(a=>a.last_read_ok_at).filter((s):s is string=>Boolean(s)).sort().at(-1);
    text=last?`Email last checked ${new Date(last).toLocaleString('en-GB',{dateStyle:'medium',timeStyle:'short'})}.`:'Waiting for the first successful email check.';
    if(accounts.some(a=>a.last_error))text+=' A recent check had a problem; see Settings.';
  }
  return <p className="muted" aria-label="Email status">{text} {showLink && <a href="#/settings">Connection</a>}</p>;
}
