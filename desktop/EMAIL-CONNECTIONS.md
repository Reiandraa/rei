# One email connection screen

Open Settings -> Connections -> Connect email.

- Enter your email address and password for a supported password-based IMAP service.
  REI discovers encrypted settings automatically. Some services use an app password
  or a separate campus sign-in ID; REI asks for that only when needed.
- Use Continue with Google or Continue with Microsoft for those accounts, including
  university accounts hosted by them. Sign in on the provider's official browser page.
- After a successful sign-in, REI enables the automatic Inbox flow. Tasks still wait
  for approval in Today. Checks run while REI is open and the computer is awake.

Users never enter a server, port, tenant or application ID. Password sign-in supports
IMAP LOGIN over verified TLS on port 993 when a supported configuration is found
in Thunderbird's provider directory. Only the domain is sent to that directory.
REI does not guess servers or fall back to unencrypted connections. Not every email
provider supports this method; outside-app restrictions and campus policies apply.

Existing HIZ accounts retain their configuration, secure sign-in and selected
folders. A generic new connection starts with the latest 50 Inbox messages. Google
starts with the latest 50 and follows history; Microsoft reads Inbox delta pages.
Stored messages can remain when disconnected; removing an OAuth token locally does
not revoke the provider grant. Users can revoke that in their provider account.
Messages are read only: REI does not send mail or mark server messages as read.
Attachments and remote content are not downloaded for Gmail summaries.

## Current activation status

The connectors and one-screen flow are implemented and tested with synthetic data.
The checked-in publisher client IDs are empty: Google/Microsoft sign-in is not yet
activated or tested with real accounts. The interface explains this without asking
users for developer settings. The public beta1 download is unchanged.

The publisher must complete EMAIL-PUBLISHER-SETUP.md, rebuild, and validate an actual
sign-in/fetch/reopen/disconnect on Windows and macOS before announcing availability.
These changes do not enable Teams, cloud sync or a shared server mailbox.

Message content stays local when local Ollama is selected. Choosing an optional
hosted AI provider separately can send relevant message content to that provider,
subject to the explicit AI settings consent. Email login does not enable an AI API.
