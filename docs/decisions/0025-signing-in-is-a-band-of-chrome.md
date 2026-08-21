# ADR 0025 — The browser is a station, and signing in is a band of chrome

Status: accepted · 2026-08-21

## Context

ADR 0013 decided the auth model in full, and the server implements all of it:
stations, users, PINs, scrypt, eight-hour sessions, throttling, groups,
`requirePermission`, 401 versus 403, an audit row for every refusal.

The browser implements none of it. It sends no `x-prodmesh-station` header and
no bearer token, and there is nowhere in the console to sign in.

That was harmless while every server was unconfigured — `requirePermission`
lets everything through until the first user exists. Then someone added a user
through the Users page built in M6e. Permissions came on for good, every write
started answering 401, and the console had a lock with no keyhole. The one
message the operator could reach said to sign in as somebody who holds
`graphics.users.manage`, which was true, unactionable, and the only thing on the
screen.

An enforcement boundary with no client half is not a security posture. It is a
server that can only be administered by whoever set it up first, until they
forget the PIN.

## Decision

### The browser registers itself as a station, automatically

On first load, once, keeping the token in local storage. It grants nothing —
ADR 0013 is explicit that a station identifies *where*, never *what may be
done* — so registering by default costs nothing but a row.

It has to be automatic, and the reason is the trusted station. ADR 0013's one
deliberate weakening is a booth machine or an unattended display that operates
with **nobody signed in**. A station that came into existence only when somebody
signed in there could never be marked trusted, because the case is precisely
that nobody ever signs in there.

The name is provisional — `Chrome · macOS` — and Configure renames it. A list
of stations called "Unknown" helps nobody, and asking a wall display to name
itself asks a question of a room with no keyboard.

### Anonymous stays read-only. It is not a hole; it is the design

Once users exist, an anonymous station can read and cannot write. Every mutating
route already requires a permission, and no read is gated except administration.

Locking reads too was considered and is not built. A wall display showing what
is on air needs no credentials and should be given none (ADR 0013's own words),
and a booth screen that goes blank at 10:15 on Sunday because a session expired
is the outage this whole model is shaped to avoid. **Signed out is a state that
is stated** — the footer says the station can watch, not change — rather than
one discovered by pressing something and reading a refusal.

### Signing in is a band of chrome, not a modal and not a route

ADR 0020 says nothing floats. Here that rule earns its keep rather than merely
being obeyed: **a refusal can arrive mid-edit.** A modal that steals focus, or a
route change that swaps out the workspace, would unmount an unpublished Template
in order to ask for a PIN. The band pushes the console down by one toolbar
height, takes a username and a PIN, and hands back the exact screen the operator
was on.

It opens two ways: from the identity control in the header, which is present
whenever the server has users, and automatically when the server refuses
something — carrying the refusal's own sentence, because the server already
names the missing authority and re-wording it here would make two copies that
can disagree.

**Someone already signed in and refused is short of an authority, not of a
session.** The band says so and asks for a different account, rather than
sending them round the loop they just failed.

### One copy of identity, held by the shell

The shell fetches `/api/auth/me`, publishes it on a context, and nothing else
asks. `can(permission)` **answers yes while identity is unknown**: the request
has not landed, and a control that starts disabled and enables a moment later
reads as broken. Guessing wrong this way costs a clear sentence from the server;
guessing wrong the other way makes a permitted operator believe they are locked
out mid-service. The server is the boundary — this is guidance.

### A forgotten PIN has a way back, and its authority is physical

`npm run reset-pin -- <username> <pin>` on the machine holding the database.
The first user turns permissions on for good and is made an Administrator so
that securing the server does not lock it (ADR 0013); the case that remained
unrecoverable was that user forgetting the PIN, with nobody left who could grant
anything.

There is **no HTTP route** to it, and that is the point. Physical access to the
server is the authority, the same reasoning ADR 0013 uses for a booth being a
controlled space. It drops every session the user holds, because the reason to
run it is either a rotation or somebody who should not be signed in any more.

## The alternatives, and why they lose

**A login wall in front of everything.** The operator's instinct, and it reads
as the safe answer. It loses to the wall display: it makes an unattended screen
impossible without an account whose session must never expire, which is a
password taped to a monitor. It also contradicts ADR 0013's trusted station,
which exists so a booth can work when the volunteer who knows the PIN is not
there. If this ever runs somewhere less physically controlled, a server setting
that requires a session even to read is the shape to add — see the Seam.

**A modal dialog**, as ProdMesh-Control uses. Rejected on ADR 0020, and on the
mid-edit case above, which the dashboard does not have.

**A sign-in route.** A place rather than a float, and it satisfies the layout
law, but arriving at it unmounts whatever mode was open — the same lost
Template by a different mechanism.

**Registering the station on first sign-in.** Fewer rows, and it makes the
trusted unattended display unreachable.

## Consequences

- Every request from the browser carries the station header and, when there is
  one, the bearer token. `useQuery` and `api.ts` are the only two places that
  send, so this is two functions and not a call-site audit.
- Signing in or out invalidates the whole read cache. It has to: every cached
  answer was to a question asked as somebody else.
- A dead session is dropped rather than sent forever — otherwise every 401 after
  eight hours reads as "you are not allowed" instead of "sign in again".
- The station name is a placeholder until somebody edits it, and a station list
  full of `Chrome · macOS` is a mild embarrassment rather than a failure.
- Local storage holds a bearer token. On a shared booth machine, "sign out" is
  a real action an operator has to take, and it is in the header.
- **The SSE stream stays anonymous.** `EventSource` cannot send headers, and
  `/api/stream` is a read. That is consistent today — reads are open — and it is
  the constraint that makes "require a session even to read" a real piece of
  work rather than a flag: the stream would need its token in the query string.
- **Anything that talks to the server goes through `src/lib/api.ts`.** Two
  uploads in Design had built their own `fetch` for raw bytes; they sent no
  credentials and rendered `the server answered 401` in place of the server's
  own sentence. `postBytes` exists so an upload is not a second convention.

## Seam

- **Require a session even to read.** One server setting and one guard in front
  of the read routes and the stream. Deliberately not built: it would blank the
  wall display, and nobody has asked for it in a building where the booth is
  locked. This is the first thing to add if this ever runs outside one.
- **Idle sign-out** on a shared station, which is a station property and not a
  server one — a booth should not sign out mid-service, a laptop should.
- **An identity provider** (ADR 0013's rejected ProdMesh-Control delegation)
  would replace the sign-in band's contents and nothing else. The station half
  is unaffected, which is most of why it is a separate concept.
- **Per-station default mode** — a wall display that comes up in Show and cannot
  reach Design. A station property, and it needs no new identity concept.
