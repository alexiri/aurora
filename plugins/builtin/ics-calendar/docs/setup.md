# Connect a calendar with its secret link (ICS)

Most calendar providers can publish a calendar as a **link** that anyone holding it can read. Aurora
can use that link to give an agent **read-only** access to the calendar, with no OAuth app and no
administrator involved. You give the connection a name and paste the link; the Aurora gateway keeps
it. The agent never sees the link: it asks the gateway for the calendar and the gateway fetches
exactly the link you entered.

## The link is a password

Anyone who has the link can read the calendar, so:

- **Publish the least detail you need.** Most providers offer *busy only*, *titles and locations*
  or *all details*. Choose the smallest that answers what the agent must know. With *busy only* the
  agent sees "Busy" entries and no titles.
- **Revoking is done at the provider.** *Unpublish* or *Reset* the link there, then connect again
  with the new link (the connection's *Reconnect*). Until you do, the old link keeps working.
- **Changes arrive with the provider's own delay.** Providers refresh a published calendar every
  few minutes to a few hours, and Aurora keeps a downloaded copy for five minutes, so a change just
  made may not show at once.

Aurora stores the link encrypted, shows only its host on the connection, and never returns it. An
agent gets neither the link nor its address: only the calendar's content.

## Where each provider keeps the link

This is guidance only; Aurora has no code per provider and any `https://` (or `webcal://`) link to
an `.ics` calendar works.

- **Outlook on the web / new Outlook:** *Settings → Calendar → Shared calendars → Publish a
  calendar*. Pick the calendar and the level of detail, publish, and copy the **ICS** link (not the
  HTML one). An organization may have turned publishing off: then only the *Microsoft 365* app with
  an administrator's consent can reach the calendar.
- **Google Calendar:** *Settings → the calendar → Integrate calendar → Secret address in iCal
  format*. Reset it there to revoke it.
- **iCloud:** share the calendar as a *Public Calendar* and copy its `webcal://` link.
- **Proton Calendar:** *Settings → Calendars → the calendar → Share with anyone → Create link*; pick
  the permissions that fit and copy the link.
- **Fastmail:** *Settings → Calendars → the calendar → Export/Share → Share a read-only link*.
- **Nextcloud:** *Calendar → the calendar's share icon → Share link*; add `?export` to the link for
  the ICS.
- **Exchange on-premises (OWA):** *Settings → Calendar → Shared calendars → Publish a calendar* and
  copy the ICS link, as in Outlook on the web.

## Connecting

Choose *Calendar link (ICS)* under *Apps*, give the calendar a name (for example "Private
calendar") and paste the link. Aurora checks the link once, fetching it the way an agent will, and
refuses it with a clear reason when it is not `https`, points at an address Aurora does not reach,
no longer exists (it was reset) or does not serve a calendar. The name becomes the connection's
label. Then grant the connection to the agents that may read it, like any other app.

A server on a private network works only when the instance administrator has allowed that address
range (`gateway.allowed_private`). Aurora never reaches cloud metadata or its own internal addresses.

## What the agent can do

Read the whole calendar, recurring events included. There is no write access: the link only reads.
To let an agent send invitations, use the calendar invitation tools; to create or change events,
connect the provider's own app (*Microsoft 365* or *Google Workspace*) with an OAuth client.
