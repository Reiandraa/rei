// Test-only AEAD: node:crypto's audited chacha20-poly1305 (RFC 8439, same construction as the Rust
// core). @types/node is not installed, so the used surface is declared minimally below.
import { createCipheriv, createDecipheriv } from 'node:crypto';
import type { Aead } from '../../src/pwa/core';

interface Cipher {
  setAAD(a: Uint8Array): void;
  update(d: Uint8Array): Uint8Array;
  final(): Uint8Array;
  getAuthTag(): Uint8Array;
  setAuthTag(t: Uint8Array): void;
}
const cipher = createCipheriv as unknown as (a: string, k: Uint8Array, n: Uint8Array, o: object) => Cipher;
const decipher = createDecipheriv as unknown as (a: string, k: Uint8Array, n: Uint8Array, o: object) => Cipher;

const cat = (...p: Uint8Array[]) => {
  const out = new Uint8Array(p.reduce((n, x) => n + x.length, 0));
  let o = 0;
  for (const x of p) {
    out.set(x, o);
    o += x.length;
  }
  return out;
};

export const testAead: Aead = {
  seal(key, nonce, aad, pt) {
    const c = cipher('chacha20-poly1305', key, nonce, { authTagLength: 16 });
    c.setAAD(aad);
    const ct = cat(c.update(pt), c.final());
    return cat(ct, c.getAuthTag());
  },
  open(key, nonce, aad, sealed) {
    const d = decipher('chacha20-poly1305', key, nonce, { authTagLength: 16 });
    d.setAAD(aad);
    d.setAuthTag(sealed.slice(sealed.length - 16));
    return cat(d.update(sealed.slice(0, sealed.length - 16)), d.final());
  },
};

export const hex = (b: Uint8Array) => Array.from(b, (x) => x.toString(16).padStart(2, '0')).join('');
export const unhex = (s: string) => Uint8Array.from(s.match(/../g) ?? [], (h) => parseInt(h, 16));
