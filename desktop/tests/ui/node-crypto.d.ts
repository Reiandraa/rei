declare module 'node:crypto' {
  export function createCipheriv(algorithm: string, key: Uint8Array, iv: Uint8Array, options?: object): unknown;
  export function createDecipheriv(algorithm: string, key: Uint8Array, iv: Uint8Array, options?: object): unknown;
}
