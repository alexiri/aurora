# Connect Microsoft 365

This package connects **Microsoft Graph** (`graph.microsoft.com`, API version v1.0) so agents can
work with Outlook mail and calendars. Aurora adds the credentials
to requests your agents send; agents never see the token.

## Register the Entra application (admins)

1. In the Microsoft Entra admin center open *App registrations* and add a registration.
2. Choose the supported account types. *Accounts in this organizational directory only* (single
   tenant) is fine; *any organizational directory* (multitenant) lets people from other
   directories sign in too.
3. Add a **Web** redirect URI (not *Single-page application* or *Mobile and desktop*):
   {{aurora.callback_url}}
4. Under *Certificates & secrets* add a client secret and copy its **value** (not its ID).
5. Under *API permissions* add only the delegated Microsoft Graph permissions you want to offer
   (see below). If your organization does not let people consent to apps themselves, an
   administrator has to *Grant admin consent* on that page before anyone can sign in.
6. Enter the Application (client) ID and the secret value in Aurora under
   {{aurora.page:oauth_clients}}. For a single-tenant registration also enter the **Directory
   (tenant) ID** from the registration's *Overview* page (a domain such as
   `contoso.onmicrosoft.com` works too); leave it empty for a multitenant registration. A
   workspace owner can register a client for their own workspace on the app's setup page,
   {{aurora.app_setup}}, instead.

Aurora uses the authorization code flow with PKCE and refreshes the token automatically
(`offline_access`).

## Permissions

Read access is offered by default; each write permission is a separate, clearly marked choice.

- Read (default): `User.Read`, `Mail.Read`, `Calendars.Read`, and `offline_access`.
- Write: `Mail.Send`, `Calendars.ReadWrite`.

Register only these. File permissions (`Files.*`, `Sites.*`) are not used: OneDrive and
SharePoint are not offered, and those permissions often need an administrator's consent.

## What agents can do

Reading is allowed by default: your profile, mail folders, messages and attachment lists, and
calendars and events (including time-range views).

Changing things is a separate permission and is held for approval: sending mail and creating
calendar events. The approval shows the recipients and subject, or the event subject and start
time.

## Limits

- Only the v1.0 API of the signed-in user (`/me`) is covered: mail, calendar and profile. Files
  (OneDrive, SharePoint), the beta API, other users' resources and other hosts are not.
- The Graph `$batch` endpoint is not offered: one request can mix reads and writes, so it cannot
  be classified.
- Sending drafts, replying, deleting or moving mail, updating or deleting events, sharing links
  and Teams are not offered.
- Sign-in uses the sign-in app's directory (tenant) ID, or the multitenant `common` endpoint when
  none is set. One sign-in app serves one directory; people from another directory need a
  multitenant registration.
