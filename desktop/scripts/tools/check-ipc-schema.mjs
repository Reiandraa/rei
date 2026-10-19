// Fails when the TypeScript command list drifts from schemas/ipc-v1.json.
// The Rust side is checked against the same file in src-tauri/tests/integration.rs.
import { readFileSync } from 'node:fs';

const schema = JSON.parse(readFileSync(new URL('../../schemas/ipc-v1.json', import.meta.url), 'utf8'));
const src = readFileSync(new URL('../../src/ipc.ts', import.meta.url), 'utf8');
const m = src.match(/COMMANDS = \[([^\]]*)\]/);
if (!m) throw new Error('COMMANDS not found in src/ipc.ts');
const ts = m[1].split(',').map((s) => s.trim().replace(/'/g, '')).filter(Boolean).sort();
const declared = Object.keys(schema.commands).sort();
if (JSON.stringify(ts) !== JSON.stringify(declared)) {
  console.error('IPC drift\n TS:', ts, '\n schema:', declared);
  process.exit(1);
}
console.log(`IPC schema OK (${declared.length} commands, ipc_version ${schema.ipc_version})`);
