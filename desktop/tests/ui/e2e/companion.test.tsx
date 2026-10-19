// P14 companion UI against the real PwaSession (SYNTHETIC data, node chacha20-poly1305 as test AEAD).
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { hex, testAead as aead } from '../test-aead';
import { Companion } from '../../../src/pwa/Companion';
import { sealSnapshot, type Envelope } from '../../../src/pwa/core';

const enc = new TextEncoder();
const key = new Uint8Array(32).fill(9);
const NOW = Date.parse('2026-10-19T10:00:00Z');

async function fixtures(): Promise<{ offer: Envelope; snapshot: Envelope }> {
  const base = await crypto.subtle.importKey('raw', enc.encode('1234'), 'HKDF', false, ['deriveBits']);
  const pk = new Uint8Array(await crypto.subtle.deriveBits({ name: 'HKDF', hash: 'SHA-256', salt: enc.encode('rei-os/v1'), info: enc.encode('pair') }, base, 256));
  const nonce = crypto.getRandomValues(new Uint8Array(12));
  const secret = { account_id: 'a', device_id: 'p', epochs: { '1': hex(key) }, current_epoch: 1 };
  const ct = aead.seal(pk, nonce, enc.encode('rei-sync/v1|pair|a|p|0|offer'), enc.encode(JSON.stringify(secret)));
  const offer = { op_id: 'pair-offer', account_id: 'a', device_id: 'p', epoch: 0, nonce: hex(nonce), ciphertext: hex(ct) };
  const snapshot = sealSnapshot(aead, key, 'a', 'd', 1, 's', {
    generated_at: new Date(NOW).toISOString(),
    tasks: [{ id: 't', revision: 1, created_at: '', updated_at: '', origin_device_id: 'd', deleted_at: null, title: 'Synthetic task', status: 'todo' }],
    events: [{ id: 'e', title: 'Synthetic class' }],
    briefing: null,
  });
  return { offer, snapshot };
}

describe('Companion UI', () => {
  it('wrong code shows an error and stays locked; right code shows data, quick add, stale and lock', async () => {
    const { offer, snapshot } = await fixtures();
    let t = NOW;
    render(<Companion aead={aead} offer={offer} snapshot={snapshot} now={() => t} />);
    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Pairing code'), '9999');
    await user.click(screen.getByText('Unlock'));
    expect(await screen.findByRole('alert')).toHaveTextContent(/wrong/i);
    await user.clear(screen.getByLabelText('Pairing code'));
    await user.type(screen.getByLabelText('Pairing code'), '1234');
    await user.click(screen.getByText('Unlock'));
    expect(await screen.findByText('Synthetic class')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('Up to date');
    await user.type(screen.getByLabelText('New task'), 'Call landlord');
    await user.click(screen.getByText('Add task'));
    expect(screen.getByText(/Call landlord \(todo\)/)).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('1 change(s) not yet sent');
    await user.click(screen.getByText('Lock'));
    expect(screen.getByRole('alert')).toHaveTextContent(/Unsent edits/);
    t += 3_600_000;
    await user.click(screen.getByText('Complete Synthetic task'));
    await user.click(screen.getByText('Discard and lock'));
    await waitFor(() => expect(screen.getByLabelText('Pairing code')).toBeInTheDocument());
    expect(screen.queryByText('Synthetic class')).toBeNull();
  });

  it('without a provider the companion explains it cannot unlock', async () => {
    const { offer } = await fixtures();
    render(<Companion aead={null} offer={offer} snapshot={null} />);
    const user = userEvent.setup();
    await user.type(screen.getByLabelText('Pairing code'), '1234');
    await user.click(screen.getByText('Unlock'));
    expect(await screen.findByRole('alert')).toHaveTextContent(/not installed/);
  });
});
