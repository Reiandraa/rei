import { useState, type FormEvent } from 'react';

/** Provider discovery is local. No password, DNS lookup, account or browser login. */
export function EmailSetup({onHiz}: {onHiz: (suggestedId: string) => void}) {
  const [email, setEmail] = useState('');
  const [unsupported, setUnsupported] = useState(false);
  function identify(e: FormEvent) {
    e.preventDefault();
    const [local, domain, extra] = email.trim().split('@');
    if (!local || extra !== undefined || !['uni-saarland.de','stud.uni-saarland.de'].includes(domain?.toLowerCase())) { setUnsupported(true); return; }
    // HIZ aliases need not be their account ID. Only prefill the known ID shape;
    // the owner can correct it before any credentials leave the app.
    onHiz(/^[a-z]{3}\d{5}$/i.test(local) ? local : '');
  }
  return <div className="email-setup">
    <form onSubmit={identify} aria-label="Find email connection">
      <label className="field">University email<input type="email" autoComplete="email" value={email} onChange={e=>{setEmail(e.target.value);setUnsupported(false);}} placeholder="you@your-university.de" required maxLength={254}/></label>
      <button className="primary" type="submit">Continue</button>
    </form>
    {unsupported && <div className="email-provider-notice" role="status"><strong>This email provider is not available yet</strong><p>This version connects Saarland University HIZ email: @stud.uni-saarland.de and @uni-saarland.de. Other universities, Gmail and Outlook accounts are not connected by this setup yet.</p><p>No account was connected or password requested. You can keep using the planner and add a class notice in Messages.</p></div>}
    <p className="muted">REI checks the address on this computer to choose a supported connection. No sign-in starts until you continue to its login form. Browser login is not available for the current HIZ connector.</p>
    <button className="email-id-link" type="button" onClick={()=>onHiz('')}>I already have a HIZ sign-in ID</button>
  </div>;
}
