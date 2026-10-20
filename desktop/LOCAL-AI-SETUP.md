# Guided AI setup, Windows and macOS

Owner approved shared onboarding after the first Mac beta opened but Ollama was
not installed there. Keep one source and build separately per platform.
Email onboarding is described separately in EMAIL-CONNECTIONS.md.

Settings -> AI keeps local Ollama / OpenAI / Claude choices visible. The local
path distinguishes disconnected service, missing installation, empty model list,
missing selected model, and an installed selected model. Startup recovery goes
directly to this setup, without blocking the planner. Unknown setup data does
not prevent the existing installed-model picker from working.

Installer action opens only ollama.com's platform download page. It does not
install software silently. Open action uses the standard native app location;
no renderer-provided executable, shell, URL, model registry or path is accepted.
macOS instructions explain dragging Ollama to Applications. Windows instructions
explain the installer and Start menu. Focus/visibility and four-second polling
refresh setup while the AI settings panel is mounted.

Read OS physical RAM (no serial number or telemetry); recommend Qwen3.5 2B below
12 GB/unknown, 4B at 12-23 GB, 9B from 24 GB. This is a conservative starting
point, not a GPU benchmark or guarantee of performance/summary accuracy. The
three catalog options expose approximate sizes and allow the owner to choose.
Existing installed names and manual model controls remain available.

Download starts only on an explicit click, through fixed loopback /api/pull,
with a catalog allowlist, redirects and environment proxy disabled, finite
network timeouts, bounded NDJSON lines, one job per app, and cancellation.
Progress is for the current file, never a fabricated total percentage. Completion
also requires /api/tags to list the requested model. The user then selects the
installed model; no automatic inference or provider switch happens on download.
Cancelled/failed files may remain in Ollama for later resume. No API billing,
profile/credential reads, model inference or real inbox testing is performed by
development tests. API provider consent and vault storage remain unchanged.

Sources checked 2026-10-20:
- https://docs.ollama.com/macos
- https://docs.ollama.com/api/pull
- https://docs.ollama.com/api/tags
- https://ollama.com/library/qwen3.5

Build macOS locally with `bash scripts/package-mac-beta.sh` on Apple Silicon
after checking out the published setup revision. It preserves the preview app
identifier. The script creates an ad-hoc signed app/ZIP, not notarization. Owner
must test that new binary and upload its ZIP/checksum; old Mac beta acceptance
does not validate the new setup implementation.

Validation: local setup backend tests, synthetic interface tests, lint, production
frontend build, IPC contract, SQLite migration and connection regression checks
pass. No real model download, provider login, inbox read or paid API request was
used by these checks. Native installer/open/download acceptance and Mac packaging
still need device testing. The Mac script could not be syntax-checked in this
Windows sandbox because Git Bash was blocked; no security setting was changed.
