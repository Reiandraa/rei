# REI OS desktop beta

A local-first study planner: email, important updates, proposed tasks, repeating
classes and your day in one place.

- Product: https://reiandraa.com/projects/rei/
- Browser demo with sample data: https://reiandraa.com/projects/rei/demo/
- Versioned beta downloads: https://github.com/Reiandraa/rei/releases

## Current scope

Today, one-time tasks, repeating schedules, a month calendar, message reading,
source-linked briefings and explicit task approval are implemented. Today shows
up to three important updates and links to all remaining updates in Assistant.
The AI never approves tasks merely by producing a summary.

Connect email uses one screen: secure email/password discovery or official
Google/Microsoft browser sign-in. HIZ remains supported. The owner reports
successful Google and Microsoft sign-in on Windows. Google remains External/Testing
for publisher-approved testers; public verification is not complete. Microsoft
organizations may require administrator consent or a verified publisher. Not
every university or email provider permits outside applications.

Automatic checks and reading run while REI is open and the computer is awake.
Use local Ollama and an installed model, or explicitly enable a supported hosted
AI with your own API key. Hosted AI transmits relevant content to the chosen
provider and may incur charges there; the request limit is not a dollar cap.
AI quality and speed depend on the model. Teams and cloud sync are not enabled
product features in this beta.

## Builds

Windows is an unsigned standalone x64 GUI executable with embedded frontend and
sprout icon, using WebView2. No Node.js, Rust or command prompt is needed to run it.
The existing dev.rei.os.preview profile is retained; close older REI windows first.
No user profile, email password, API key or refresh token is shipped in downloads.
Native apps contain the publisher Desktop registration; its private JSON remains
outside source control.

Apple Silicon Mac builds run locally on the owner's MacBook. They are ad-hoc
signed and not Apple-notarized. A Mac download is published only after that build
is opened and tested. No Mac CI is used.

Install Node.js 20.19+ or 22.12+, npm, Rust stable and Tauri 2 prerequisites for
your OS. macOS needs Xcode command-line tools; Windows needs Microsoft C++ Build
Tools and WebView2. Development does not require Claude Code or paid launchers.

For development from this desktop folder:

```sh
npm ci
npm run tauri -- dev
```

For the owner Google-enabled Windows build:

```powershell
powershell -File scripts/build-google-windows.ps1 -CredentialsPath <private-Desktop-JSON>
```

For the owner Apple Silicon beta:

```sh
bash scripts/package-mac-beta.sh "$HOME/Downloads/REI-Google-Desktop.json"
```

The publisher must privately supply their own downloaded Google Desktop JSON,
matching email-publisher.json. The Mac wrapper validates it before running checks,
then builds the arm64 app and packages only the app and checksum. No private JSON
is included in that ZIP. A native build without the Desktop credential refuses
Google sign-in before opening a browser. Do not use another app's credentials.

## Checks and limits

```sh
npm run lint
npm run build
npm run test:unit
npm run test:e2e
node scripts/tools/check-ipc-schema.mjs
node scripts/tools/prepare-test-fixtures.mjs
cargo test --manifest-path src-tauri/Cargo.toml --locked --tests
```

Current Windows checks: 171 UI cases collectively pass, 44 Rust library tests and
10 core integration tests pass, lint/build/Clippy and the 63-command IPC check pass.
Full network-transport testing was blocked by the local execution environment's
socket permissions, not counted as passed. The Mac script runs the full synthetic
suite before packaging. The real-model smoke test is opt-in and remains ignored.

The test-fixture script creates a local test-only RSA key and matching public JWKS.
The key is ignored and never uploaded; it is not used by the desktop app.
Tests do not prove every mailbox, provider billing behavior, or real-model quality.
Google and Microsoft acceptance is owner-reported; saved refresh/restart and current
Mac native acceptance require separate checks.

See [beta 2 review](BETA-2-REVIEW.md), [email setup](EMAIL-CONNECTIONS.md),
[publisher setup](EMAIL-PUBLISHER-SETUP.md) and [local AI setup](LOCAL-AI-SETUP.md).
This public source export omits inboxes, profiles, credentials, cost ledgers,
private builder prompts, development transcripts and generated build artifacts.