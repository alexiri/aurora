# Connect Microsoft 365

This package connects **Microsoft Graph** (`graph.microsoft.com`, API version v1.0) so agents can
work with Outlook mail, calendars and OneDrive or SharePoint files. Aurora adds the credentials
to requests your agents send; agents never see the token.

## Register the Entra application (admins)

1. In the Microsoft Entra admin center open *App registrations* and add a registration.
2. Choose supported account types that include *any organizational directory* (and *personal
   Microsoft accounts* if you want to connect those). Aurora signs in through the multi-tenant
   `common` endpoint, so a single-tenant registration is not accepted by Microsoft.
3. Add a **Web** redirect URI: `<your Aurora URL>/v1/oauth/callback`.
4. Under *Certificates & secrets* add a client secret and copy its **value**.
5. Under *API permissions* add only the delegated Microsoft Graph permissions you want to offer
   (see below). Permissions such as `Sites.Read.All` usually need tenant admin consent.
6. Enter the Application (client) ID and the secret value in Aurora under *Admin, Apps,
   Microsoft 365*.

Aurora uses the authorization code flow with PKCE and refreshes the token automatically
(`offline_access`).

## Permissions

Read access is offered by default; each write permission is a separate, clearly marked choice.

- Read (default): `User.Read`, `Mail.Read`, `Calendars.Read`, `Files.Read`, and `offline_access`.
- Read (optional): `Sites.Read.All` for SharePoint sites and document libraries.
- Write: `Mail.Send`, `Calendars.ReadWrite`, `Files.ReadWrite`.

## What agents can do

Reading is allowed by default: your profile, mail folders, messages and attachment lists,
calendars and events (including time-range views), your OneDrive files and folders, file
downloads, and SharePoint sites and document libraries.

Changing things is a separate permission and is held for approval: sending mail, creating
calendar events, creating OneDrive folders and uploading or replacing file content. The approval
shows the recipient and subject, the event subject and start time, or the folder name and item.

## Limits

- Only the v1.0 API of the signed-in user (`/me`) and SharePoint sites and drives by id are
  covered. The beta API, other users' resources and other hosts are not.
- Address OneDrive files by item id. Path-addressed items such as `/me/drive/root:/folder/file`
  are not available, so new files cannot be created by path; replace the content of an existing
  item or create a folder.
- File downloads answer with a redirect to a pre-authenticated storage URL on another host,
  which this connection does not cover.
- The Graph `$batch` endpoint is not offered: one request can mix reads and writes, so it cannot
  be classified.
- Sending drafts, replying, deleting or moving mail, updating or deleting events, sharing links
  and Teams are not offered.
- Sign-in uses the `common` endpoint; tenant-specific endpoints (single-tenant registrations)
  are not supported by this package.
