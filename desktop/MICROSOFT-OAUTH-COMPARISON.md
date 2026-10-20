# Microsoft email activation — independent developer comparison

Status: 20 October 2026. Owner resumed Microsoft work after reporting Google login success with Windows build dd65ad64e9fe82db4aa1741675e83b3f60ee19740e5b272a73c2810525276e31. Google success is owner-reported; refresh/restart/new-mail acceptance has not yet been recorded.

References inspected:
- https://github.com/Foundry376/Mailspring/blob/master/app/internal_packages/onboarding/lib/onboarding-constants.ts
- https://github.com/simonrob/email-oauth2-proxy/discussions/301
- https://github.com/simonrob/email-oauth2-proxy/issues/43
- https://learn.microsoft.com/en-us/entra/identity-platform/quickstart-register-app
- https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow

The independent proxy's maintainer reproduced successful personal Hotmail authorization using an owned Entra registration. Its real issue discussion identifies a /common endpoint versus consumer-only audience mismatch; both work/school and personal accounts must be selected for REI. Its separate issue 43 establishes that a configured Microsoft public native client omits client_secret. These observations agree with REI's current system-browser PKCE, /common authorization and token endpoints, native localhost callback and no Microsoft secret. Mailspring confirms a public desktop architecture, but its client ID, IMAP/SMTP/contact/calendar scopes and code were not copied.

REI uses Microsoft Graph delegated User.Read and Mail.Read plus offline_access, not the proxy's broad IMAP/POP/SMTP permissions. Its supported-account setting must be AzureADandPersonalMicrosoftAccount. Redirect is Mobile and desktop applications -> http://localhost/callback; never register it as Web or SPA. Add no application-wide permissions or client secrets. Capture the resulting REI-owned public Application (client) ID in email-publisher.json only after actual registration succeeds.

Publisher registration completed on 20 October 2026 after the owner completed Azure signup and expressly approved Microsoft Platform Policies. REI OS Desktop Beta is Activated in the owned directory 89df1869-cd30-4a99-b715-d13f5a6a9d53 with public client ID fbf7688d-0ed0-41bb-851e-fba690601f6b, AzureADandPersonalMicrosoftAccount audience and the native http://localhost/callback redirect. The portal confirms only delegated User.Read and Mail.Read. No admin consent, private Microsoft secret, paid upgrade, billable resource or real mailbox read was performed. Billing overview confirms free credit expires in 30 days and amount due EUR 0.00. The owner strictly requires free only. New unverified multitenant publishers can face user-consent restrictions; native personal/campus sign-in acceptance is still required. This replaces the former Microsoft Services directory blocker, not proof of working mailbox access.

Added two Microsoft-specific Rust regression checks for /common, localhost redirect, PKCE, minimal Graph read scopes, no secret, account identity binding and qualified Mail.Read. All 12 cloud-email unit tests pass. No actual Microsoft mailbox read or new binary/release was performed. Google source configuration and latest working EXE are retained.

Release sequence requested by owner:
1. Complete owned Microsoft publisher registration and native email acceptance.
2. Finish the four requested points (their exact titles are being clarified; do not invent them).
3. Run full Windows validation, including native sign-in/read/restart/disconnect acceptance where the owner approves.
4. Build and test the same revision locally on the owner's Apple Silicon MacBook; no Mac CI. Carry publisher Desktop credentials via an owner-approved private transfer, never public GitHub or chat.
5. Publish versioned Windows/macOS beta downloads on GitHub and website only for binaries verified on the corresponding platform. Do not replace beta1 assets with different bytes or advertise unbuilt Mac downloads.
