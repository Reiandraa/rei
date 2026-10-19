import type { Aead } from './core';

/**
 * Production AEAD provider. Intentionally unavailable until the approved audited library
 * (@noble/ciphers chacha20poly1305, ADR-013) is installed; installs need user authorization.
 * Failing closed beats shipping an unreviewed cipher.
 */
export const productionAead: Aead | null = null;
