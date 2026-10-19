// Synthetic IPC promises exercise reload/inference races; never calls a real model or account.
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { CalendarScreen } from '../../../src/screens/MonthCalendar';
import { dateKey } from '../../../src/schedule';
import { setTransport, type CalEvent } from '../../../src/ipc';
import { syntheticBackend } from '../fake-backend';

afterEach(()=>{cleanup();setTransport(null);});
const month=()=>dateKey(new Date()).slice(0,7);
const day=(n:number)=>`${month()}-${String(n).padStart(2,'0')}`;
const event=(id:string,title:string,n:number):CalEvent=>({id,revision:1,title,all_day:true,start_local:day(n),end_local:day(n+1),start_utc:null,end_utc:null,tz:'Europe/Berlin',provider:null,read_only:false});
const cell=(n:number)=>screen.getByRole('region',{name:'Month calendar'}).querySelector(`time[datetime="${day(n)}"]`)!.closest('button')!;
const hasLabel=(n:number,label:string)=>expect(within(cell(n)).getByText(label,{exact:true})).toBeInTheDocument();

it('keeps School and Work when adding a trip, while only the changed date falls back during inference and failure',async()=>{
  const be=syntheticBackend();const events=[event('lecture','Algorithms lecture',19),event('shift','Work shift',20)];
  let reads=0;let reject!:(reason:unknown)=>void;
  setTransport(async(cmd,args)=>{
    if(cmd==='event_list')return events.map(e=>({...e}));
    if(cmd==='event_create'){const req=args!.req as {title:string;start_local:string;end_local:string};events.push({...event('trip',req.title,21),start_local:req.start_local,end_local:req.end_local});return events.at(-1);}
    if(cmd==='calendar_day_labels'){
      reads++;
      if(reads===1)return {state:'ready',labels:{[day(19)]:'School',[day(20)]:'Work',[day(21)]:'Free'}};
      return new Promise((_resolve,refuse)=>{reject=refuse;});
    }
    return be.transport(cmd,args);
  });
  const user=userEvent.setup();render(<CalendarScreen/>);
  await waitFor(()=>hasLabel(19,'School'));hasLabel(20,'Work');hasLabel(21,'Free');
  await user.click(screen.getByRole('button',{name:'+ Add event'}));await user.type(screen.getByLabelText('Event title'),'Trip to Frankfurt');fireEvent.change(screen.getByLabelText('Date'),{target:{value:day(21)}});await user.click(screen.getByRole('button',{name:'Save event'}));
  await screen.findByText('Event saved.');await waitFor(()=>expect(reads).toBe(2));
  hasLabel(19,'School');hasLabel(20,'Work');hasLabel(21,'Planned');expect(cell(21)).toHaveTextContent('Trip to Frankfurt');
  await act(async()=>reject({code:'Unavailable',message:'Synthetic local model is busy'}));
  await screen.findByText(/AI labels are unavailable right now/);hasLabel(19,'School');hasLabel(20,'Work');hasLabel(21,'Planned');
});

it('invalidates the label for an edited day while preserving another date through a partial busy response',async()=>{
  const be=syntheticBackend();const events=[event('lecture','Lecture',19),event('shift','Work shift',20)];
  let reads=0;let finish!:(value:unknown)=>void;
  setTransport(async(cmd,args)=>{
    if(cmd==='event_list')return events.map(e=>({...e}));
    if(cmd==='calendar_day_labels'){
      reads++;
      if(reads===1)return {state:'ready',labels:{[day(19)]:'School',[day(20)]:'Work'}};
      return new Promise(resolve=>{finish=resolve;});
    }
    return be.transport(cmd,args);
  });
  render(<CalendarScreen/>);await waitFor(()=>hasLabel(19,'School'));hasLabel(20,'Work');
  events[0]={...events[0],title:'New work appointment',revision:2};
  fireEvent(window,new Event('rei-assistant-changed'));await waitFor(()=>expect(reads).toBe(2));
  hasLabel(19,'Planned');hasLabel(20,'Work');expect(cell(19)).toHaveTextContent('New work appointment');
  await act(async()=>finish({state:'busy',labels:{[day(20)]:'Work'}}));
  hasLabel(19,'Planned');hasLabel(20,'Work');
});
