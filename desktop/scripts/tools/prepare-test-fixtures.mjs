// Generates only the local mock-server JWT fixture. Never used by the app.
import { generateKeyPairSync, createPrivateKey, createPublicKey } from 'node:crypto';
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const directory = new URL('../../src-tauri/tests/fixtures/', import.meta.url);
const privatePath = new URL('synthetic-test-rsa.pem', directory);
const publicPath = new URL('synthetic-test-jwks.json', directory);
mkdirSync(directory, { recursive: true });
let privateKey;
if (existsSync(privatePath)) {
  privateKey = createPrivateKey(readFileSync(privatePath));
} else {
  privateKey = generateKeyPairSync('rsa', { modulusLength: 2048 }).privateKey;
  writeFileSync(privatePath, privateKey.export({ type: 'pkcs8', format: 'pem' }),
    { flag: 'wx', mode: 0o600 });
}
if (privateKey.asymmetricKeyType !== 'rsa') throw new Error('Expected a synthetic RSA fixture');
const publicKey = createPublicKey(privateKey).export({ format: 'jwk' });
writeFileSync(publicPath, JSON.stringify({ keys: [{ ...publicKey,
  use: 'sig', kid: 'synthetic-test-key', alg: 'RS256' }] }, null, 2) + '\n');
console.log('Synthetic JWT test fixtures ready in ' + fileURLToPath(directory));
console.log('Private fixture stays local and ignored. Do not upload it.');
