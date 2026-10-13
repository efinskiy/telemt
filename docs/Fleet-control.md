# Fleet control and adaptive shaping

See [deployment and rollback](../deploy/fleet/README.md) and
[measured validation](../deploy/fleet/VERIFICATION.md). The independent manifests
are `admin/backend/Cargo.toml`, `agent/Cargo.toml` and
`tools/shaping-harness/Cargo.toml`; no root workspace migration is required.
Build with `cargo build --locked --manifest-path PATH`. Build the UI with
`npm ci --prefix admin/frontend` and `npm run build --prefix admin/frontend`.
The backend serves its build from `ADMIN_ASSETS_DIR`; development UI/API must
use one HTTPS origin because cookies and write origins are deliberately strict.

## Control contracts

The browser calls only `/api/*` on the central service. Each mutating request
except login requires the session cookie, exact HTTPS Origin and the session's
`X-CSRF-Token`. Sessions expire after eight hours and can be revoked by disabling
or changing the administrator; logout removes the server-side session. TOTP
steps cannot be replayed. Password hashing uses Argon2id. Owner-only physical
recovery requires the matching database and vault key.

| Resource | Contract |
| --- | --- |
| `POST /api/login`, `GET /api/session`, `POST /api/logout` | Password/TOTP login, session recovery and revocation |
| `/api/administrators` | Owner enrollment and role/enable changes; last owner protected |
| `/api/nodes` | Owner enroll/edit/delete; HTTP/HTTPS API origin using public/private IPv4, IPv6 or DNS; encrypted individual token/test key |
| `GET /api/nodes/{id}/read?path=...` | Explicit allowlisted Telemt GET, including its revision |
| `GET /api/nodes/{id}/history` | Up to 1,440 newest minute averages within 30 days |
| `/api/operations` | Durable multi-node mutation intent with a revision per node |
| `POST /api/operations/{id}/reconcile` | Read back uncertain writes; never automatically resend |
| `/api/agents`, `GET /api/probes` | Owner enroll/disable probes, monitor network/family results |
| `/api/groups` | Owner enrollment, reserve order/token updates, retirement and automation toggle |
| `/api/groups/{id}/preview`, `/switch`, `/history` | Preview, manually apply, and inspect verified outcomes |
| `/api/groups/{id}/records`, `/repair`, `/reconcile` | Owner current-record acknowledgement and uncertain-write recovery |
| `/api/groups/{id}/verify-configuration` | Explicit new compatibility baseline after fleet credential changes |
| `GET /api/audit` | Bounded recent audit, credential values redacted |

Operations require `node_ids`, `method`, `path`, `body` and a `revisions` object
keyed by node UUID. Every target is recorded before the first write. Node
conflicts remain failures; no implicit overwrite or write retry occurs.
`succeeded` means confirmed terminal completion, not merely an HTTP 202.
An interrupted operation records unattempted targets separately. Known accepted
reloads resume polling after restart; other ambiguous states require readback.
A config save, runtime activation and reload completion are separate states.
Only shaping saves use hot application, preserving established connections.

`owner` manages central accounts, enrollment and DNS. `operator` manages proxy
users, links, editable node config and runtime actions. `viewer` reads monitoring
without tokens, user links or credential-bearing upstream URLs. The node's own
validated API remains the authority for editable fields and revision conflicts.

## Local shaper

`[shaping]` is optional and off by default. State lives in the process, outside
configuration generations. One normalized IPv4 /32 or IPv6 /128 owns the budget
across users and connections; mapped IPv6 normalizes to IPv4. Admission occurs
after existing trusted-client address resolution. 32,768 retained IPs and 65,536
logical flows are the default bounds. Exhaustion rejects admission explicitly.

Successful reads and writes settle independent upload/download reservations;
unused bytes are refunded on partial operations, pending I/O and cancellation.
Existing static user/CIDR limits still apply. Encrypted framing, KDF and fallback
are untouched. The disabled path checks a process atomic and forwards existing
transport I/O without timer allocation. Enabled timers are allocated once and
reused. Registry locks occur at admission/control calculation, not byte copying.

A one-second control tick maintains a 60-slot rolling window and weighted
max-min allocation. Weights vary by at most 16:1 based on IP minute usage relative
to equal channel share. Flow bulk hysteresis is 256 KiB/64 KiB. A quarter of each
direction is reserved for small demand, shared by IP; unused capacity can be
borrowed. Borrowing beyond an IP's priority grant uses the bulk scope, preventing
new connections from multiplying priority. A global directional budget sits
above the IP budget. Epochs are 20 ms; when the whole channel allows only one
byte per epoch, outstanding small demand takes precedence over bulk.

`bulk_up_bps` and `bulk_down_bps` are independently configurable contention caps.
Zero retains the percentage-derived default. Explicit nonzero values must be at
least 400 bits/s and no larger than the directional capacity. The effective bulk
cap is the smaller of that value and the channel remainder after
`priority_percent`; the rest is guaranteed to pending small demand. These are
shared class caps, not per-connection speed limits. The admin shaping page saves
them through the revision-checked hot configuration API.

For example, a 100 Mbit/s download channel with `bulk_down_bps = 20000000`
reserves at least 80 Mbit/s for pending small traffic. Bulk can use the entire
100 Mbit/s when that traffic does not need it. A fully served small request
releases its unused reserve immediately; pending small demand reclaims capacity
by the following epoch. Already transmitted bytes cannot be recalled. Demand
expires after a short idle interval and is refreshed by actual reservation
attempts. Stable calculated class shares survive unrelated IP arrivals/departures.

Classification uses encrypted flow volume (default 256 KiB entry, 64 KiB exit
within 60 seconds), not content, user intent or a guaranteed distinction between
messages and files. Authenticated handshake bytes use the small-flow budget;
unauthenticated public fallback retains its existing behavior. A reconnect starts
a small logical flow but shares the retained IP's history, weight and class
budgets. Thresholds and contention caps should be tuned from observed load.

Controller calculations are generation-fenced, independent of static binding
publication. Pacing is excluded from relay idle deadlines; genuine peer stalls
still use the existing idle policy. All authenticated Direct, Middle and WEB
relays use the same limiter. Payload order is unchanged. Restart resets the
minute history; reconnect and config reload do not.

`GET /v1/runtime/shaping?offset=0&limit=100` exposes the current policy, retained
IPs, committed directional bytes, wait/refusal counters, calculation age and IP
window/rate/class/grants. Limit is 1..1,000, offset at most 131,072. Existing node
metrics include aggregate `telemt_shaping_*` values. `GET /v1/fleet/traffic`
provides counters and their availability when per-user telemetry is disabled;
`GET /v1/fleet/compatibility` provides a secret-free fingerprint of users,
enabled state, modes, effective listener ports/transports and TLS domains.

## Probes and DNS

Agents use individual hashed tokens, one-use job IDs and controller-owned network
identities. Up to 200 assignments run concurrently, with bounded responses and
five-second MTProto deadlines; controller/reference HTTPS requests have a
two-second limit to fit the 14-second round result lifetime. Public reachability
requires an authenticated FakeTLS flight, obfuscated padded-intermediate channel
and a valid `resPQ` fingerprint vector with the original nonce. Management,
public proxy and upstream/readiness are presented separately; the UI does not
attribute a regional failure to censorship without independent evidence.

Only owner-enrolled `RU` sites enter quorum; the owner verifies their actual
location/provider independence. Quorum counts distinct registered networks and excludes unhealthy references.
Unknown/expired rounds do not accumulate failure. IPv4 and IPv6 are evaluated
separately. Consecutive thresholds are three failures, two reserve successes,
six recovery successes and a five-minute switch cooldown. Healthy active nodes
stay active; ordered reserves do not cause automatic failback.

The coordinator holds a cancellation-safe PostgreSQL transaction advisory lock
while committing a separate durable write intent. Cloudflare updates only
registered IDs, checks ownership/name/family/DNS-only state, writes TTL 60 and
reads back both families. A crash or partial/uncertain response freezes further
writes. Reconciliation reads; explicit owner repair requires the displayed
current record state. External changes pause automation. A lost database or
Cloudflare connection cannot produce a successful in-memory switch.

Monitoring samples every five seconds, with reset-aware rate deltas. Detail
expires after 24 hours; minute averages after 30 days; audit after 180 days.
Unavailable data is marked stale/unknown rather than converted to zero rates.

Group membership and DNS-token edits pause automation and keep the current active
node enrolled. Retiring a disabled, reconciled group leaves DNS unchanged and
records its last enrollment in audit. Record ownership is unique within the
zone; the five-minute cooldown is recorded by zone/record and survives group
retirement/reenrollment. Editing or disabling node enrollment pauses its groups;
verify the active configuration before reenabling automation.

## Assigned-address IP rotation

`GET /v1/runtime/interfaces` reads the node's current interface inventory and
active-generation listener configuration. It exposes at most 512 assigned
addresses, rejects an oversized enumeration, and marks down, loopback,
unspecified, multicast and link-local addresses unusable. It does not assign
addresses, call a hosting provider, or infer a public address behind NAT.

The owner creates one pool per node and one owning DNS group. A pool contains
2–8 canonical assigned addresses, including the group's current A/AAAA targets.
Single-node DNS groups are supported. Pool automatic rotation defaults to
false. The pool's owner must remain enabled; readiness and configuration drift
can suspend an enabled policy. Fleet DNS automation must be disabled for a
pool-owned group so two independent policies cannot fight over the same records.

Preparation previews the complete listener array and its source revision.
Applying it clones the active MTProxy endpoint's policy onto reserve addresses,
announces the enrolled proxy domain in links, and uses the validated config API
with `If-Match` and a 300-second drain reload. Existing user credentials are not
rewritten. Specific IP binding and disabled SYN limiting are prerequisites for
runtime rebind; wildcard bindings and SYN-limited endpoints require a deployment
change. WEB listener inventory stays unchanged.

External jobs are scoped to their exact address at issuance; a new IP cannot
inherit its predecessor's health. Scheduling issues the bounded endpoint/agent
cross product in one database statement. Up to 200 assignments per agent run
concurrently, each retaining the five-second protocol deadline. Network failure
remains unknown. A candidate needs two consecutive successful rounds from two
independent registered Russian networks and an active listener. A previously
failed address needs six successful rounds before reuse.

A manual preview captures the old IP and enrolled DNS state. Automatic rotation
additionally requires three failed rounds for the current IP, quorum in every
enrolled address family, readiness, compatibility and a five-minute cooldown.
The durable workflow is `queued → dns_pending → retiring → succeeded`:

1. Verify assigned address, active reserve listener, readiness and fresh quorum.
2. Commit the exact DNS intent and rotation ID before making a Cloudflare call.
3. Verify Cloudflare A/AAAA records. Commit the new node address in the same
   transaction as DNS verification; crash recovery follows the same path.
4. Queue a separate revision-checked listener reload to retire the previous
   MTProxy acceptor. Keep other addresses, ports and WEB listeners intact.
5. Confirm terminal reload completion and active listener inventory. Store the
   old address in quarantine and retain the complete operation/audit history.

Disabling automation cancels an automatically queued change before its DNS
write. A write already in flight requires verification or reconciliation.
Ordinary node/config mutations are fenced while a rotation is pending.
An uncertain DNS write is read back without resending. The owner can repair a
failed listener retirement from a fresh displayed revision. An explicitly
observed retirement with lost reload status records `reload_completed: null`.
External DNS drift requires exact current-record acknowledgment, which stops
automation and cancels old queued previews; a fresh manual preview follows.
Deleting a disabled, reconciled pool leaves listeners and DNS unchanged.

| Central resource | Contract |
| --- | --- |
| `/api/ip-pools` | Read pools; owner creates from freshly assigned addresses |
| `/api/ip-pools/{id}` | Owner toggles automation or removes reconciled enrollment |
| `/api/ip-pools/{id}/preview`, `/prepare` | Preview and apply complete listener preparation with displayed revision |
| `/api/ip-pools/{id}/status`, `/history` | Address-specific quorum, listener state and bounded rotation history |
| `/api/ip-pools/{id}/rotation-preview`, `/rotate` | Capture and submit a manual IP/DNS switch |
| `/api/ip-pools/{id}/reconcile`, `/cancel` | Read back uncertainty; cancel only before a DNS write |
| `/api/ip-pools/{id}/retirement-preview`, `/repair-listener` | Submit a fresh owner-approved listener repair |
| `/api/ip-pools/{id}/acknowledge-dns` | Acknowledge exactly displayed external record state |

TLS shaping sits below buffered record framing, after FakeTLS authentication.
Counters settle actual successful transport operations, including TLS framing;
a buffered record or cancelled flush is not counted as a delivered TCP write.
The same lease follows authenticated dispatch into Direct/Middle, preventing
double charging. Middle's activity epoch observes waits at this lower boundary.
WEB virtual stream writes use their existing bounded carrier queues and share
the same client-IP budget. Cipher state, KDF tuples and raw public fallback are
preserved when a transport is replaced or pacing is removed after rejection.


### Pool-owned DNS recovery

Ordinary group switch/repair writes are rejected while an IP pool owns the group.
Use `GET /api/ip-pools/:id/dns-repair-preview` and submit the exact displayed
object to `POST /api/ip-pools/:id/repair-dns`. The owner-approved repair supersedes
the uncertain DNS intent atomically, preserves the original rotation, disables
automation, and queues a new write after fresh readiness/quorum checks. Changed
records invalidate the preview. If all records already match the original intent,
including TTL, reconciliation completes it without a second write. Partial writes
remain explicitly repairable even when the new A/AAAA address is already present.
Only enrolled address families participate in the autorotation quorum.
