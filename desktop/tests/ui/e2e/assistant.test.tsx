// Synthetic UI/IPC only. No live Ollama quality or native appearance claim.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from '../../../src/App';
import { DailyReview } from '../../../src/screens/DailyReview';
import { ImportantBriefPreview } from '../../../src/screens/Assistant';
import { setTransport, type AssistantBrief, type AssistantNote } from '../../../src/ipc';
import { autoBackend, cand } from '../auto-backend';

afterEach(()=>{cleanup();setTransport(null);});
function backend(){
  const be=autoBackend();be.settings.enabled=true;
  const brief:AssistantBrief={state:'idle',enabled:true,model:'synthetic-model',updated_at:'2026-10-19T20:00:00Z',detail:'',total:3,scope_limit:100,pending:0,routine:1,entries:[
    {source_id:'source-class',subject:'Lecture cancellation',received_at:'2026-10-19T19:00:00Z',importance:'important',points:[{text:'The lecture is cancelled tomorrow.',excerpt:'Vorlesung morgen fällt aus.'}],partial:false,detail:'',updated_at:'2026-10-19T20:00:00Z'},
    {source_id:'source-unclear',subject:'Unclear registration',received_at:'2026-10-19T18:00:00Z',importance:'uncertain',points:[],partial:true,detail:'Only part of this long message was read.',updated_at:'2026-10-19T20:00:00Z'}
  ]};
  let notes:AssistantNote[]=[];
  const transport:typeof be.transport=async(cmd,args)=>{
    if(['assistant_summary_get','assistant_summary_refresh','assistant_memory','source_list'].includes(cmd))be.calls.push({cmd,args});
    if(cmd==='source_list')return [];
    if(cmd==='assistant_summary_get')return structuredClone(brief);
    if(cmd==='assistant_summary_refresh'){brief.pending=1;brief.state='queued';return structuredClone(brief);}
    if(cmd==='assistant_memory'){
      const req=args!.req as {action:string;id:string|null;text:string|null;expected_hash:string|null;confirm:boolean|null};
      if(req.action==='save'){expect(req.confirm).toBe(true);const note={id:req.id??'note',text:req.text!,hash:req.id?'hash-2':'hash-1'};notes=req.id?notes.map(n=>n.id===req.id?note:n):[...notes,note];}
      if(req.action==='delete'){expect(req.confirm).toBe(true);notes=notes.filter(n=>n.id!==req.id);}
      return structuredClone(notes);
    }
    return be.transport(cmd,args);
  };
  setTransport(transport);return {...be,brief};
}

it('requires the first explicit summarize action even when stored messages are waiting',async()=>{
  const be=backend();be.brief.enabled=false;be.brief.pending=2;be.brief.entries=[];window.location.hash='#/assistant';render(<App/>);
  expect(await screen.findByText('Choose Summarize to prepare your important-message briefing.')).toBeInTheDocument();
  expect(screen.queryByText(/Waiting for local AI/)).toBeNull();expect(be.calls.some(c=>c.cmd==='assistant_summary_refresh')).toBe(false);
  expect(screen.getByRole('button',{name:'Summarize important updates'})).toBeEnabled();
});

it('opens Assistant from Messages and queues one background refresh while keeping saved points visible',async()=>{
  const be=backend();window.location.hash='#/inbox';const user=userEvent.setup();render(<App/>);
  await user.click(screen.getByRole('link',{name:'Summarize important updates'}));
  await screen.findByRole('heading',{level:1,name:'Assistant'});
  expect(await screen.findByText('The lecture is cancelled tomorrow.')).toBeInTheDocument();
  expect(await screen.findByText(/Waiting for local AI/)).toBeInTheDocument();
  expect(be.calls.filter(c=>c.cmd==='assistant_summary_refresh')).toHaveLength(1);
  expect(screen.getByRole('button',{name:'Summarizing…'})).toBeDisabled();
  const important=screen.getByRole('region',{name:'Important updates'});expect(within(important).getByRole('link',{name:'View message →'})).toHaveAttribute('href','#/inbox?source=source-class');
  await user.click(within(important).getByText('Source quote'));expect(within(important).getByText('Vorlesung morgen fällt aus.')).toBeVisible();
  expect(screen.getByRole('region',{name:'Messages needing attention'})).toHaveTextContent('Unclear registration');
  expect(be.calls.some(c=>['task_create','candidate_batch_approve','hiz_sign_in'].includes(c.cmd))).toBe(false);
});

it('integrates cached important points with daily review while approvals still send exact displayed hashes',async()=>{
  const be=backend();be.control.actionable=[cand(1)];const user=userEvent.setup();render(<DailyReview compact/>);
  const review=await screen.findByRole('region',{name:'Daily review'});
  const preview=await within(review).findByRole('generic',{name:'Important message summary'});
  expect(await within(preview).findByText('The lecture is cancelled tomorrow.')).toBeInTheDocument();
  expect(within(preview).getByRole('link',{name:'Open summary →'})).toHaveAttribute('href','#/assistant');
  await user.click(within(review).getByText(/Review suggestions and updates/));await user.click(await within(review).findByRole('button',{name:'Approve 1 selected'}));
  await waitFor(()=>expect(be.tasks).toHaveLength(1));expect(be.control.batches[0]).toEqual([{id:'fixture-cand-1',payload_hash:'fixture-hash-1'}]);
});

it('saves only explicit owner preferences and allows revision-bound edit and confirmed deletion',async()=>{
  const be=backend();window.location.hash='#/assistant';const user=userEvent.setup();render(<App/>);
  await screen.findByText('Up to date with the messages in this briefing.');
  expect(be.calls.some(c=>c.cmd==='assistant_memory')).toBe(false);
  await user.click(screen.getByText('What REI remembers'));await screen.findByLabelText('Add a preference');
  await user.type(screen.getByLabelText('Add a preference'),'Prioritize math classes');expect(be.calls.filter(c=>c.cmd==='assistant_memory')).toHaveLength(1);
  await user.click(screen.getByRole('button',{name:'Save preference'}));await screen.findByText('Prioritize math classes');
  await user.click(screen.getByRole('button',{name:'Edit'}));await user.clear(screen.getByLabelText('Edit preference'));await user.type(screen.getByLabelText('Edit preference'),'Prioritize Spanish');await user.click(screen.getByRole('button',{name:'Save changes'}));
  expect(be.calls.filter(c=>c.cmd==='assistant_memory').at(-1)!.args).toEqual({req:{action:'save',id:'note',text:'Prioritize Spanish',expected_hash:'hash-1',confirm:true}});
  await user.click(await screen.findByRole('button',{name:'Delete: Prioritize Spanish'}));expect(be.calls.some(c=>(c.args?.req as {action?:string})?.action==='delete')).toBe(false);
  await user.click(screen.getByRole('button',{name:'Confirm delete: Prioritize Spanish'}));await waitFor(()=>expect(screen.queryByText('Prioritize Spanish')).toBeNull());
  expect(be.calls.filter(c=>c.cmd==='assistant_memory').at(-1)!.args).toEqual({req:{action:'delete',id:'note',text:null,expected_hash:'hash-2',confirm:true}});
});

it('keeps existing summaries through an offline refresh and exposes the real connection settings without resetting sign-in',async()=>{
  const be=backend();window.location.hash='#/assistant';render(<App/>);await screen.findByText('The lecture is cancelled tomorrow.');
  be.brief.state='offline';be.brief.pending=2;be.brief.detail='Local AI is unavailable. Existing summaries are kept.';
  act(()=>fireEvent(window,new Event('rei-assistant-changed')));
  expect(await screen.findByText('Local AI is unavailable. Existing summaries are kept.')).toBeInTheDocument();expect(screen.getByText('The lecture is cancelled tomorrow.')).toBeInTheDocument();
  expect(be.calls.some(c=>['hiz_sign_in','connection_disconnect','assistant_workflow_set'].includes(c.cmd))).toBe(false);
});

it('limits Today to three ranked updates and opens all ten without starting inference',async()=>{
  const be=backend();be.brief.total=12;be.brief.routine=1;
  be.brief.entries=Array.from({length:10},(_,n)=>({...be.brief.entries[0],source_id:`important-${n}`,subject:`Important ${n}`,points:[{text:`Saved update ${n}`,excerpt:`Quote ${n}`}],priority:{rank:n<2?0:n===2?1:3,reason:n<2?'Due soon':n===2?'Schedule change to review':'Important update',due_date:null}}));
  be.brief.entries.push({source_id:'failed',subject:'Needs checking',received_at:'2026-10-19T20:00:00Z',importance:'failed',points:[],partial:false,detail:'Open to check.',updated_at:'2026-10-19T20:00:00Z'});
  window.location.hash='#/today';const user=userEvent.setup();render(<App/>);
  const preview=await screen.findByRole('generic',{name:'Important message summary'});
  await within(preview).findByText('Saved update 0');
  expect(within(preview).getAllByRole('listitem')).toHaveLength(3);
  expect(within(preview).queryByText('Saved update 3')).toBeNull();
  expect(within(preview).getAllByText(/Due soon/)).toHaveLength(2);
  await user.click(within(preview).getByRole('link',{name:'7 more important updates →'}));
  const full=await screen.findByRole('region',{name:'Important updates'});
  expect(await within(full).findByText('Saved update 9')).toBeInTheDocument();
  expect(within(full).getAllByRole('article')).toHaveLength(10);
  expect(be.calls.some(c=>c.cmd==='assistant_summary_refresh')).toBe(false);
});

it('counts only completed important summaries and updates the remaining link as results change',async()=>{
  const be=backend();be.brief.pending=5;
  be.brief.entries=Array.from({length:4},(_,n)=>({...be.brief.entries[0],source_id:`saved-${n}`,points:[{text:`Point ${n}`,excerpt:'Quote'}]}));
  render(<ImportantBriefPreview/>);
  expect(await screen.findByRole('link',{name:'1 more important update →'})).toHaveAttribute('href','#/assistant');
  expect(screen.getByText(/5 still to summarize/)).toBeInTheDocument();
  be.brief.entries=be.brief.entries.slice(0,2);
  act(()=>fireEvent(window,new Event('rei-assistant-changed')));
  await waitFor(()=>expect(screen.queryByRole('link',{name:/more important update/})).toBeNull());
  expect(screen.getAllByRole('listitem')).toHaveLength(2);
});
