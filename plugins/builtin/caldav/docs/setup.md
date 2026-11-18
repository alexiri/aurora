# Connect a calendar over CalDAV

CalDAV lets an agent **read and change** a calendar on a server you choose, signing in with your
username and an **app password**: no OAuth app and no administrator are involved. It works with
iCloud, Fastmail, Nextcloud, mailbox.org, Posteo, Zimbra, SOGo, Radicale, Baikal, Synology and any
other CalDAV server.

## What the agent sees

- **The password never reaches the agent.** The Aurora gateway adds it to the agent's requests to
  your server; the agent's code can use any placeholder password.
- **The server's address is visible to the agent.** It needs it to talk to the server, and the
  password is the only secret. The agent finds your accounts in `~/.aurora/servers.json`.
- **Only your server gets the password.** Aurora sends it only to the server addresses it found
  when you connected (shown on the connection), over `https`, and never to an address another app
  or Aurora itself owns. An agent that follows a link to another host sends nothing.

## Where to get an app password

Use an **app-specific password**, never your main one, so you can revoke it alone.

- **iCloud:** server `https://caldav.icloud.com`; make the password at *appleid.apple.com →
  Sign-In and Security → App-Specific Passwords*. The username is your Apple ID.
- **Fastmail:** server `https://caldav.fastmail.com`; *Settings → Privacy & Security → Integrations
  → New app password*, access to *Calendars (CalDAV)*. The username is your full Fastmail address.
- **Nextcloud:** server `https://<your-nextcloud>` (Aurora finds `/remote.php/dav`); *Settings →
  Security → Devices & sessions → Create new app password*.
- **mailbox.org:** server `https://dav.mailbox.org`; *Settings → Security → App passwords*.
- **Posteo:** server `https://posteo.de:8443`; *Settings → My account → Apps and devices → App
  passwords*, or your normal password if you have not turned them on.
- **Any other server:** the address your provider documents for CalDAV (often the web address, or
  `https://<host>/dav`), your username and password.

## Connecting

Choose *CalDAV calendar* under *Apps*, enter the server address, your username and the app
password (a name is optional: without one the connection is labelled "username at host"). Aurora
signs in once to find your calendar home and refuses with a clear reason when:

- the address is not `https` (`caldav_not_https`);
- it points at a private or internal address, or one an organization policy blocks
  (`caldav_destination_refused`);
- nothing answers (`caldav_unreachable`);
- the server says the username or password is wrong, or that it wants an app password
  (`caldav_unauthorized`);
- it does not answer as a CalDAV server (`caldav_not_caldav`);
- the host belongs to another app or to Aurora (`server_host_claimed`).

A server on a private network works only when the instance administrator has allowed that address
range (`gateway.allowed_private`). If your provider moves your account to another server later,
requests there get no password and the server answers 401: connect again (*Reconnect*) to
discover the new address.

## Google and Outlook are not here

- **Google Calendar** needs OAuth: connect the *Google Workspace* app (`caldav_needs_oauth`).
- **Microsoft 365 and Outlook** have no CalDAV: read with the *Calendar link (ICS)* app, and write
  with the calendar invitation tools (`caldav_unsupported`).

## What the agent can do

Read calendars and events (a date range, with repeating events expanded by the server), and, with
write access, create, change, move and delete events and calendars. Changes use the server's
ETags, so an event you edited meanwhile is never overwritten: the agent re-reads and retries.

**A change that invites attendees emails them.** Servers with automatic scheduling (iCloud,
Nextcloud and others) send the invitation as soon as the event is saved. Under *Ask* access, the
approval shows the title, time, whether it repeats, the location and **every attendee** before you
decide. Sending a scheduling message is a change like any other: it needs write access.
