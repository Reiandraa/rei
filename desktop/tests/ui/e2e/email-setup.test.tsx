// Provider discovery only. No credentials, accounts, browser sessions or networking.
import { cleanup, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { EmailSetup } from '../../../src/screens/EmailSetup';

afterEach(cleanup);
it('recognizes supported HIZ addresses without starting authentication and prefills only a campus ID shape', async()=>{
  const next=vi.fn();const user=userEvent.setup();render(<EmailSetup onHiz={next}/>);
  expect(screen.queryByLabelText('Password')).toBeNull();expect(next).not.toHaveBeenCalled();
  await user.type(screen.getByLabelText('University email'),'abc12345@STUD.UNI-SAARLAND.DE');
  await user.click(screen.getByRole('button',{name:'Continue'}));expect(next).toHaveBeenCalledWith('abc12345');
});
it('does not assume a personal email alias is the HIZ sign-in ID',async()=>{
  const next=vi.fn();const user=userEvent.setup();render(<EmailSetup onHiz={next}/>);
  await user.type(screen.getByLabelText('University email'),'synthetic.alias@uni-saarland.de');
  await user.click(screen.getByRole('button',{name:'Continue'}));expect(next).toHaveBeenCalledWith('');
});
it('keeps unsupported universities out of the HIZ login and offers the planner without a fake browser connection',async()=>{
  const next=vi.fn();const user=userEvent.setup();render(<EmailSetup onHiz={next}/>);
  await user.type(screen.getByLabelText('University email'),'student@university.example');
  await user.click(screen.getByRole('button',{name:'Continue'}));
  expect(await screen.findByRole('status')).toHaveTextContent('This email provider is not available yet');
  expect(next).not.toHaveBeenCalled();expect(screen.queryByLabelText('Password')).toBeNull();
  await user.click(screen.getByRole('button',{name:'I already have a HIZ sign-in ID'}));expect(next).toHaveBeenCalledWith('');
});
