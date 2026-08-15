# ADR 0013 — Stations, users, and ACLs, inherited from ProdMesh-Control

Status: accepted · 2026-08-14

## Context

ProdMesh-Control solved this already, in its own ADR 0008, and the model is
better than what we would invent from scratch. The core insight is worth
restating because it is not obvious:

**A station is not a user.** A browser registers once as a *station* — a random
token in an `x-prodmesh-station` header — and that token grants **no
permissions**. It identifies *where* an action came from. An anonymous station is
a read-only display. Users then authenticate *contextually at a station*, with a
username and a PIN, getting a bearer token that expires in eight hours.

That separation is what lets a wall display and a booth screen be the same
software with completely different authority, and it gives an audit trail that
records both the person and the place.

There is one real difference in our situation, and it is important enough that
copying the model without thinking would produce a broken system.

**ADR 0009 says a node keeps running when the server dies, and hardware surfaces
fire without it.** An unattended Show station in a booth, or a Stream Deck on a
desk, must be able to take a graphic. If taking requires a logged-in user with an
eight-hour session, then the first Sunday somebody's session expires mid-service,
the graphics stop — and the person who could log in is running audio.

## Decision

**Inherit ProdMesh-Control's model wholesale, with one deliberate change.**

### Inherited unchanged

- Station registration, token in `x-prodmesh-station`, no permissions attached.
- Users authenticate at a station: username + PIN, scrypt-hashed, bearer token
  SHA-256-hashed in SQLite, eight-hour TTL, per-station+username throttling.
- Groups grant a union of dotted-string permissions. A built-in Administrators
  group holds the wildcard.
- `requirePermission(perm)` middleware. **401 versus 403 is meaningful** — the
  browser's only way to distinguish "nobody is logged in" from "this operator
  lacks the permission" — and every refusal carries a human `label` so the UI can
  name the missing authority.
- Every refusal is audited with user, station, action, and result.
- **The UI hides controls it would be refused, and the server is the boundary.**
  Hiding is guidance, never enforcement.

### Permissions

```
graphics.operate            take, take out, update, clear
graphics.rundown.edit       playlists, items, rundown order
graphics.templates.edit     design and publish templates
graphics.nodes.configure    outputs, formats, colour, layer stacks, adoption
graphics.assets.manage      upload, ingest, GC
graphics.settings.manage    server settings, API keys
graphics.users.manage       users, groups, stations
graphics.logs.view          take log, audit log, node telemetry detail
```

### The one change: trusted show stations

**A station may be marked *trusted for operation* on a specific set of layers or
outputs**, by someone holding `graphics.stations.manage`. A trusted station may
send `graphics.operate` cues within that scope **with no user logged in**.

This is a deliberate weakening and it is bounded on purpose:

- Trust is granted per station and scoped to named outputs or layers, never
  globally.
- It grants **only** `graphics.operate` — never editing, configuring, or
  managing.
- It is recorded in the audit log as the station acting, with no user, which is
  the honest record.
- Configure shows every trusted station in one list, so the blast radius is
  visible rather than scattered through settings.

The justification is the same one behind ADR 0009: authority that evaporates at
the worst moment is not a security control, it is an outage. A booth is a
physically controlled space, and the threat model here is a LAN appliance on a
closed show network, not the open internet.

**Hardware surfaces that terminate at the node** (GPI, MIDI, OSC — ADR 0014) are
the same principle taken to its conclusion: they are physical access, and
physical access to the booth is the control.

## Consequences

- The auth model is one an operator already knows from ProdMesh-Control, and
  `server/authStore.js` and `server/httpAuth.js` come over nearly unchanged.
- Adding a permission is a string plus a DB seed, and it inherits audit and UI
  gating for free.
- A wall display showing what is on air needs no credentials and can be given
  none.
- A booth can operate through a session expiry, a server restart, and a network
  partition.
- **Trusted stations are a real weakening** and the mitigation is visibility, not
  cleverness: scoped, listed, audited. If this ever runs somewhere less
  physically controlled, this is the first thing to revisit.
- Two identity concepts — station and user — is more than most apps have, and it
  takes a paragraph to explain to a new contributor. Worth it.

## Alternatives rejected

**A fresh auth model designed for graphics.** Rejected: no reason to invent, and
two ProdMesh apps with different login models is a real operator cost.

**Delegate authentication to ProdMesh-Control** so operators have one login.
Genuinely the best operator experience, and rejected because it creates a runtime
dependency on another server — which directly contradicts ADR 0009. A graphics
system that cannot authenticate when the dashboard is down is not autonomous. Worth
revisiting only as an *optional* identity provider with local fallback, and that
would need its own ADR.

**Require a logged-in user for every take, no exceptions.** The clean answer, and
rejected for the reason above: it converts a session expiry into a service
outage.

**No auth at all**, like the read-only satellite APIs in the family. Rejected —
those APIs are read-only, and this one changes what a congregation sees.

## Seam

Permissions are dotted strings in a table, so a productized version can add
tenancy, per-room scoping, or SSO without touching call sites.

The station/user split is also the seam for a future mobile or tablet operator
surface: it is another station, with its own trust scope, and no new concept.
