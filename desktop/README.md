# REI OS desktop preview

REI brings university email, important updates, proposed tasks, repeating
classes, and a daily planner together in a local-first desktop app.

- Product: https://reiandraa.com/projects/rei/
- Interactive sample-data demo: https://reiandraa.com/projects/rei/demo/

## Beta downloads

Open the Windows x64 `REI OS.exe` preview directly. Its frontend is embedded;
no command prompt, development server, Node.js, or Rust installation is needed
to run it. Windows WebView2 must be present. Ollama and a local model are needed
only for local AI; a configured hosted API is the other option.

The app opens with the REI sprout logo and real workspace, appearance, and AI
availability checks. Unavailable Ollama does not prevent using the planner.
This executable is unsigned and is not an installer. Windows preview binaries
are distributed in the repository's Releases section.

The Windows preview build retains the earlier `dev.rei.os.preview` profile
identifier. Existing users should close their old preview before opening it.
No private profile or credentials are included in source or binary downloads.

A macOS universal build configuration is included for development, but a Mac
download is not available in this Windows beta. Mac distribution validation
is incomplete. Its optional workflow uses no Apple credentials; any future
beta would use an ad-hoc signature and would not be notarized.

## Current scope

Today, one-time tasks, repeating schedules, a monthly calendar, message reading,
source-linked important briefings, and explicit task approval are implemented.
The current university connector is HIZ IMAP over TLS. Automatic checks and
analysis run while the desktop app is open and the computer is awake.

Ollama is the default local AI runtime. Optional Claude and OpenAI adapters use
your own API key, stored in the operating system's credential store. Enabling
an API requires explicit provider/model consent: relevant message content is
sent to that provider and its account may be charged. A request-count limit is
provided; it is not a guaranteed dollar cap.

This is a development preview, not a finished or signed release. Teams
connectors and other experimental modules remain in the source, but live
Teams access, cloud sync, and a full conversational assistant with searchable
memory are not offered as working product features. Downloads are beta previews; native acceptance testing is still in progress.

## Run from source

Install Node.js 20.19+ (or 22.12+), npm, stable Rust, and the prerequisites for
Tauri 2 on your operating system. Windows requires Microsoft C++ Build Tools
and WebView2; macOS requires Xcode command-line tools. Ollama is optional when
using a configured hosted API instead. Development does not require Claude
Code or a paid development launcher.

From this `desktop` directory:

```sh
npm ci
npm run tauri -- dev
```

The frontend runs on port 1420. `npm run dev` alone is only the web frontend;
the native Rust backend is needed for real accounts and saved data. Configure
email and AI locally in Settings. Never put passwords, API keys, or a copied
application profile into the repository.

To create a native build for your current platform:

```sh
npm run tauri -- build
```

For the standalone Windows preview with the earlier preview profile identifier:

```sh
npm run tauri -- build --no-bundle --config tauri.windows-preview.conf.json
```

For the universal macOS beta (on a Mac with both Rust targets installed):

```sh
npm run tauri -- build --target universal-apple-darwin --bundles dmg --config tauri.macos-beta.conf.json
```

Developer ID signing, notarization, and native platform acceptance remain pending.

## Checks

```sh
npm run lint
npm run build
npm run test:unit
npm run test:e2e
node scripts/tools/check-ipc-schema.mjs
cargo fmt --manifest-path src-tauri/Cargo.toml --check
node scripts/tools/prepare-test-fixtures.mjs
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

The fixture preparation script generates a new local test-only RSA key and its
matching public JWKS. The private fixture remains ignored and is never uploaded.
It is used only by the mocked Teams transport tests, never by the desktop app.

Tests use synthetic data and local/mock transports. Some transport tests need
permission to open local sockets. Historical local validation passed 149 UI
tests and 92 offline Rust tests; three socket-dependent Ollama tests were
filtered in that environment. Real OS-vault operations, live provider
networking/billing, real-model quality, and all release platforms are not
verified by those synthetic results. The latest startup slice passed all 154 UI
tests and built an optimized Windows GUI executable. Static executable checks
confirmed the embedded frontend, six sprout icon frames, and GUI subsystem.
Native visual acceptance remains unverified.

## Layout

`src/` contains the React/TypeScript interface. `src-tauri/` contains the Rust
backend, migrations, permission declarations, and backend tests. `schemas/`
holds the IPC contract, and `tests/ui/` holds synthetic interface tests.

This public source export omits personal inboxes, databases, credentials,
development transcripts, cost ledgers, builder prompts, and build artifacts.
It preserves runtime source and synthetic tests from local app commit
`c22de588c8aea639e031dcdc9ee1a7d3517302da`. The export's package scripts omit
the private development-phase evidence launchers. Dependency versions and
runtime implementation are unchanged. The earlier Python prototype remains
at the repository root.
