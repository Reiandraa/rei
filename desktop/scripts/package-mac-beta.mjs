// Owner-local build. Private Desktop JSON stays outside source and release files.
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve, dirname, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
function run(command, args, env = process.env) {
  const result = spawnSync(command, args, { cwd: root, env, stdio: 'inherit' });
  if (result.error || result.status !== 0) throw new Error(`${command} did not complete successfully. Nothing was published.`);
}
try {
  const checkOnly = process.argv[2] === '--check-credential';
  const credentialPath = process.argv[checkOnly ? 3 : 2];
  if (!credentialPath) throw new Error('Pass your private Google Desktop JSON path: bash scripts/package-mac-beta.sh /path/to/client_secret.json. Keep this file outside the repository.');
  const published = JSON.parse(readFileSync(resolve(root, 'email-publisher.json'), 'utf8').replace(/^\uFEFF/, ''));
  let document;
  try { document = JSON.parse(readFileSync(resolve(credentialPath), 'utf8').replace(/^\uFEFF/, '')); }
  catch { throw new Error('Cannot read the private Desktop JSON. Credential values are not logged.'); }
  const installed = document.installed;
  if (!installed || document.web) throw new Error('Use the publisher Google Desktop JSON, not a Web or service-account credential.');
  if (installed.client_id !== published.google_client_id) throw new Error('The Desktop JSON belongs to a different application. No build started.');
  if (typeof installed.client_secret !== 'string' || !/^[A-Za-z0-9._~-]{16,256}$/.test(installed.client_secret)) throw new Error('The Desktop credential is missing or malformed. No build started.');
  if (checkOnly) {
    console.log('Desktop credential matches REI. No build, account access, or upload performed.');
  } else {
    if (process.platform !== 'darwin' || process.arch !== 'arm64') throw new Error('Build on the owner Apple Silicon Mac. No Mac CI or cross-platform readiness claim.');
    const targetSetting = process.env.CARGO_TARGET_DIR;
    const target = targetSetting ? (isAbsolute(targetSetting) ? targetSetting : resolve(root, targetSetting)) : resolve(root, 'src-tauri/target');
    const env = { ...process.env, CARGO_TARGET_DIR: target, REI_GOOGLE_DESKTOP_CLIENT_SECRET: installed.client_secret };
    console.log('Building REI with its own Google and Microsoft registrations. Credential values are not printed.');
    run('npm', ['ci']);
    run('node', ['scripts/tools/prepare-test-fixtures.mjs']);
    run('npm', ['run', 'lint']);
    run('npm', ['run', 'test:unit']);
    run('npm', ['run', 'test:e2e']);
    run('node', ['scripts/tools/check-ipc-schema.mjs']);
    run('cargo', ['test', '--manifest-path', 'src-tauri/Cargo.toml', '--locked', '--tests']);
    try { run('npm', ['run', 'tauri', '--', 'build', '--target', 'aarch64-apple-darwin', '--bundles', 'app', '--config', 'tauri.macos-beta.conf.json'], env); }
    finally { delete env.REI_GOOGLE_DESKTOP_CLIENT_SECRET; }
    const app = resolve(target, 'aarch64-apple-darwin/release/bundle/macos/REI OS.app');
    run('codesign', ['--verify', '--deep', '--strict', app]);
    run('lipo', [resolve(app, 'Contents/MacOS/rei-os'), '-verify_arch', 'arm64']);
    const out = resolve(root, 'beta-artifacts');
    mkdirSync(out, { recursive: true });
    const name = 'REI-OS-macOS-Apple-Silicon-beta2.zip';
    run('ditto', ['-c', '-k', '--sequesterRsrc', '--keepParent', app, resolve(out, name)]);
    const hash = createHash('sha256').update(readFileSync(resolve(out, name))).digest('hex');
    writeFileSync(resolve(out, 'SHA256-macOS-Apple-Silicon.txt'), `${hash}  ${name}\n`);
    console.log(`Built and signature-checked: ${app}\nZIP/checksum: ${out}\nOpen and test the app before publishing. Ad-hoc signed; not Apple-notarized.`);
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : 'Mac packaging failed. No release was published.');
  process.exitCode = 1;
}
