# Publisher activation: Google and Microsoft email

These steps are performed once by the REI publisher, not by each user. The app's
Connect email screen contains no developer registration fields. No registration,
real login or provider verification has been completed by the development checks.

## Google

1. In https://console.cloud.google.com/ create/select the REI project you own.
2. Enable Gmail API. Configure Google Auth Platform branding, publisher contact,
   audience and data access. Add the read-only scope:
   `https://www.googleapis.com/auth/gmail.readonly`.
3. Create an OAuth client of type **Desktop app**, for the shared Windows/macOS app.
   REI uses system-browser authorization, PKCE S256 and a random-port IPv4 loopback
   callback `http://127.0.0.1:<port>/callback`.
4. Put its public client ID into `email-publisher.json` -> `google_client_id`.
   Do not put passwords, refresh tokens or web client secrets into this repository.
   The native flow uses the documented optional-client-secret PKCE exchange.
5. In testing mode, add the intended testers. Public availability of Gmail read-only
   access requires the relevant Google verification process; testing access is not
   a public release. Review testing-mode refresh-token expiry before promising
   persistent connections. Provide accurate app homepage, privacy and deletion
   information, including optional hosted AI use, and follow the applicable policy.

## Microsoft

1. In https://entra.microsoft.com/ use App registrations -> New registration.
   Select organizational directories **and personal Microsoft accounts** for both
   Microsoft 365 universities and Outlook.com. Set publisher/contact/branding.
2. Add a Mobile and desktop applications / public native client redirect:
   `http://localhost/callback`. REI uses a random port; Microsoft documents that
   localhost redirect matching ignores that port. The native listener binds only
   IPv4 loopback, never a network interface. Do not register this as a web or SPA
   app and do not create a client secret for this native flow.
3. Add Microsoft Graph **delegated** permissions `User.Read` and `Mail.Read`.
   REI additionally requests `offline_access` for refresh tokens. No send/write or
   application-wide mailbox permissions are requested.
4. Put Application (client) ID into `email-publisher.json` -> `microsoft_client_id`.
   `/common` is supplied by the app; users do not enter tenants. Campus consent
   rules can still require an administrator, and cannot be bypassed by REI.

Client IDs identify a public installed app and are not credentials. They may be
checked into source. Alternatively set REI_GOOGLE_CLIENT_ID and/or
REI_MICROSOFT_CLIENT_ID in the build environment; these are compile-time values.
Never share actual passwords, tokens or mailbox dumps in chat or source control.

## Build and verify before enabling the download

Build Windows from the same published source and build Apple Silicon locally on
Mac using `bash scripts/package-mac-beta.sh`. No Mac CI is required. Keep the existing
preview profile identifier. Test with a consenting test mailbox: cancel sign-in,
complete sign-in, reject a mismatched account, read Inbox, receive another message,
restart and fetch using saved sign-in, disconnect, and confirm future access stops.
Use provider account settings to revoke grants when needed. Do not claim native
acceptance or OAuth verification solely from synthetic tests. Do not overwrite an
already published beta executable with different bytes under the same filename.

## Primary references checked 2026-10-20

- https://developers.google.com/identity/protocols/oauth2/native-app
- https://developers.google.com/workspace/gmail/api/auth/scopes
- https://developers.google.com/identity/protocols/oauth2/policies
- https://learn.microsoft.com/en-us/entra/identity-platform/reply-url
- https://learn.microsoft.com/en-us/graph/permissions-reference#mailread
- https://wiki.mozilla.org/Thunderbird:Autoconfiguration
