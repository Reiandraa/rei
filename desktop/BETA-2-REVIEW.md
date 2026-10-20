# REI OS beta 2 review — 20 October 2026

Google and Microsoft sign-in use the provider's official system-browser pages.
Other email uses encrypted provider discovery; the existing Saarland HIZ setup is
retained. Sign-in credentials stay in the operating system credential store.

The owner reports successful Google and Microsoft sign-in on Windows. This is
not proof of all providers, every university's consent policy, saved refresh over
weeks, or Mac acceptance. Google is still External/Testing: intended Gmail testers
must be allowed by the publisher. Microsoft organizations can require admin consent
or a verified publisher. Do not promise unrestricted Gmail or campus access.

Advanced settings contain Time zone and Diagnostics. AI model, Ollama setup and
optional API controls live in the AI tab. Browser sign-in can be cancelled both
for new connections and when signing in again to a saved account.

Daily review shows at most three important updates, with a link to the rest in
Assistant. Summaries remain tied to original messages and do not approve tasks.
Local AI quality and latency depend on the selected model and hardware; synthetic
tests do not prove the output quality of a user's installed model.

The beta runs only while REI is open and the computer is awake. Local Ollama and
optional user-paid AI APIs are separate choices. No Azure compute/server is used
by Microsoft browser login. Cloud sync and Teams are not enabled in this release.

Windows is a standalone x64 GUI executable using WebView2 and the existing
dev.rei.os.preview profile. It is not code-signed. Mac is Apple Silicon only,
built locally by the owner, ad-hoc signed and not Apple-notarized. Publish a Mac
download only after the owner builds and opens this exact source revision.

For Mac, run from the desktop source folder:

```sh
bash scripts/package-mac-beta.sh "$HOME/Downloads/REI-Google-Desktop.json"
open "src-tauri/target/aarch64-apple-darwin/release/bundle/macos/REI OS.app"
```

The private JSON is supplied separately by the publisher, outside this repository.
The script packages beta-artifacts/REI-OS-macOS-Apple-Silicon-beta2.zip and its
SHA256 checksum. No CI, model download, account login, paid API call or upload is
started by this script. Native acceptance is a separate owner action.
