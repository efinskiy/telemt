# Telemt fleet deployment

This directory builds the modified node, control service, probe and UI from this
checkout. PostgreSQL 16 persists control state; the proxy and its shaper never
require the control service to forward traffic. Run on Linux with Docker Engine
and Compose v2. Rust builders use MSRV 1.88. The administration domain must be
separate from the proxy's TLS camouflage domain.

## Prepare the controller

1. Copy `.env.example` to `.env`; replace the domains and node directory. Keep the
   independently reachable probe reference outside the controller's network.
2. Point the administration domain at the controller. Open TCP 80/443 for HTTPS
   and certificate issuance. Establish a private VPN route to each node's API;
   the backend accepts only literal private or loopback management addresses.
3. Create secrets in this directory:

```sh
umask 077
mkdir -p secrets
openssl rand -hex 32 > secrets/database_password
openssl rand -hex 32 > secrets/master_key
printf 'postgresql://telemt:%s@postgres:5432/telemt\n' "$(cat secrets/database_password)" > secrets/database_url
openssl rand -hex 32 > secrets/probe_token
openssl rand -base64 24 > secrets/bootstrap_password
sudo chown 10001:10001 secrets/master_key secrets/database_url secrets/probe_token secrets/bootstrap_password
chmod 600 secrets/*
docker compose -f compose.yml build admin
docker compose -f compose.yml up -d postgres
```

`probe_token` is replaced with the one-time token produced when enrolling the
actual probe. No probe is started by default. Keep the 32-byte master key outside
the database backup and outside Git. Losing it loses encrypted node/DNS/TOTP
credentials; changing it is not a password reset.

Create the first owner through physical CLI access. Capture the returned TOTP
secret into an authenticator and remove the bootstrap password afterward:

```sh
docker compose -f compose.yml run --rm --no-deps \
  -v "$PWD/secrets/bootstrap_password:/run/secrets/bootstrap_password:ro" \
  admin create-owner owner /run/secrets/bootstrap_password
rm secrets/bootstrap_password
```

Migrations run before CLI commands and before the service starts. The CLI refuses
to bootstrap another owner once an enabled owner exists. Account recovery uses
`reset-owner USER PASSWORD_FILE` with the same database and master key, creates a
new TOTP secret, revokes existing sessions and records an audit event.

## Issue and renew HTTPS

Start the HTTP-only certificate bootstrap, issue the certificate, then replace
it with HTTPS. Export the real values used in `.env` for the certificate command:

```sh
export ADMIN_DOMAIN=admin.example.org
export ACME_EMAIL=operations@example.org
docker compose -f compose.yml -f compose.http.yml up -d admin nginx
docker compose -f compose.yml --profile certificate run --rm certbot certonly \
  --webroot -w /var/www/acme --non-interactive --agree-tos \
  --email "$ACME_EMAIL" -d "$ADMIN_DOMAIN"
docker compose -f compose.yml up -d --force-recreate nginx
```

Install `renew.service` and `renew.timer` after changing their checkout path if
it is not `/opt/telemt`. Enable the timer with systemd. `renew.sh` tests Nginx
configuration before reloading it. Review timer failures and certificate expiry.
PostgreSQL and the administration HTTP port are not published to the host.

## Enroll a node

Deploy the modified Telemt binary or build the node profile on each node host:

```sh
docker compose -f compose.yml --profile node build node
docker compose -f compose.yml --profile node up -d --no-deps node
```

The node uses host networking and its own writable, root-owned config directory.
Its isolated state volume holds runtime files. Do not share a state directory
between instances. Configure API access on the private interface, for example:

```toml
[server.api]
enabled = true
listen = "10.70.0.2:9091"
whitelist = ["10.70.0.1/32", "10.70.0.2/32"]
auth_header = "Bearer REPLACE_WITH_INDIVIDUAL_RANDOM_TOKEN"
minimal_runtime_enabled = true
```

Bind/whitelist values depend on the VPN. Include the controller and the node's own management address for the container liveness check. Registration stores the API token and
test secret encrypted and does not write or replace any existing proxy keys.
Create a dedicated test user on the node; use its 16-byte hexadecimal secret,
actual public IPs, public port, TLS SNI and DC in the Servers enrollment form.
The same credentials must already work on each reserve. Existing user links
remain the operator's responsibility when preparing compatible nodes.

Enable shaping through the validated configuration API after measuring usable
channel capacities. These are bits per second, not bytes per second:

```toml
[shaping]
enabled = false
up_bps = 100000000
down_bps = 100000000
priority_percent = 25
bulk_enter_bytes = 262144
bulk_exit_bytes = 65536
max_ips = 32768
```

Removing the section preserves old behavior. Enabling without both capacities
is rejected. A shaping-only save uses hot application without draining existing
connections. The UI waits until the runtime policy matches; a general config
save follows the node's reload operation. Deployment-only parameters are marked
in the configuration view. Table exhaustion rejects new IP admissions; retained
IP history lasts 60 seconds, including reconnects, and restarts reset that history.

## Install independent probes

Enroll at least two agents in separate Russian provider networks. Country must be explicitly registered as `RU`; the owner verifies the actual
provider network and site. Network labels are controller-owned; using two machines on the same network does not establish
independent quorum. Put each issued token in that host's `secrets/probe_token`,
owned by its probe UID. Configure `ADMIN_DOMAIN` and an independently reachable
HTTPS reference, then run on each external host:

```sh
docker compose -f compose.yml --profile probe build probe
docker compose -f compose.yml --profile probe up -d --no-deps probe
```

The agent polls HTTPS and probes only controller-enrolled literal addresses. Its
TCP/FakeTLS/obfuscation/MTProto exchange has a five-second overall deadline; only
an authenticated `resPQ` with the requested nonce counts as reachable. A failed
reference marks that network unknown. Probe tokens and test-user keys must be
rotated by reenrollment when a probe host is replaced.

## DNS enrollment and release order

Use a Cloudflare token restricted to DNS Edit on the registered zone. Enroll only
explicit A/AAAA record IDs, with DNS-only records. The ordered node list defines
reserve priority; the group editor can reorder/add/detach reserves and rotate the
DNS token. Saving pauses automation. Retire a disabled, reconciled group to
unregister it without changing DNS; record cooldown survives reenrollment. The controller saves the observed DNS and compatibility
fingerprint and verifies every write. TTL is set to 60 during a switch. It never
changes arbitrary records or enables Cloudflare proxying.

1. Release one node with shaping and DNS automation disabled.
2. Enable shaping with measured capacities; watch rate, wait and exhaustion data.
3. Add compatible reserves and independently authenticated probes.
4. Preview and test a manual switch in a dedicated test zone.
5. Enable automation last. Require three failed rounds from two networks,
   two successful reserve rounds and fresh readiness. A quarantined node needs
   six successful rounds. Cooldown is five minutes; there is no automatic failback.

Missing quorum, any unknown enrolled address family, no eligible reserve,
external record drift or an uncertain write freezes automation. Reconcile reads
Cloudflare without repeating a write. For a genuine external/partial conflict,
inspect current records and use the owner's explicit repair action with that
exact preview. After changing proxy credentials or TLS compatibility, groups
are paused; verify their new configuration baseline before reenabling automation.
Open TCP sessions are not migrated by DNS and client caches can delay new use.

## Backup, restore and rollback

`./backup.sh /safe/path/telemt.dump` makes a restrictive PostgreSQL custom dump.
Back up the master-key file separately to access-controlled storage and test
restores. The dump includes encrypted credentials, sessions and audit. Keep node
config/state backups independently; database backup does not copy those mounts.
`./restore.sh /safe/path/telemt.dump` stops the cabinet and reverse proxy, restores
transactionally and starts them again. Proxy nodes continue serving independently.

Before upgrading, disable DNS automation, back up the database/key and each node
config, preserve the previous source-built images with unique release tags, and
record the Git revision. Rebuild and replace the controller first; migrate
nodes in stages. To roll back, stop the cabinet, restore the pre-upgrade database
with its matching key, load the preserved image tags, and start admin/Nginx.
Review `unknown` operations and DNS records before reenabling automation. Do not
roll an old node binary onto a config containing unsupported new sections;
restore its matching config first.

| Symptom | First cause to check |
| --- | --- |
| Cabinet returns 503 | PostgreSQL or key-file permission; database never falls back to memory |
| Node row stale/unreachable | VPN routing, API token, whitelist, node restart |
| Config save conflicts | `If-Match` revision changed; reread and compare before resubmitting |
| Operation unknown/not sent | Lost response or controller restart; read back, do not repeat blindly |
| New clients rejected | IP/flow admission capacity, existing static limits |
| DNS conflict/unknown | External change or partial Cloudflare write; owner repair/reconcile |
| Probe network unknown | Agent host/reference failure or expired assignments |
| Healthy reserve incompatible | User secrets/enabled state, modes, port or TLS domains differ |

## Rotate assigned IPs on one server

The host must already own the candidate addresses on enabled interfaces, with
routing and firewall access to the public MTProxy port. The cabinet never adds
OS addresses or replaces a hosting provider allocation. Use the host-network
node profile so the node API enumerates host interfaces.

Configure an explicit MTProxy listener on the current assigned address with
SYN limiting off before enabling runtime rotation. The following addresses and
domains are examples to replace in the deployment configuration:

```toml
[[server.listeners]]
ip = "192.0.2.10"
port = 443
transport = "mtproxy"
synlimit = "off"
announce = "proxy.example.org"
```

A wildcard listener cannot be kept open while binding its specific reserves
with the existing socket policy. WEB listener addresses are deployment-owned.
Rebind requests involving SYN limiting or WEB inventory changes are rejected;
prepare those deployment settings and restart the node first.

1. Enroll a DNS group pointing to the node's current IP. A group may contain one
   node for local IP rotation. Leave fleet DNS automation off.
2. Open **IP rotation → Create IP pool**. Select the server, its DNS group and
   2–8 usable assigned interface addresses. Include current A/AAAA targets.
3. Preview listener preparation, review the full array and captured revision,
   then apply it. Watch the preparation operation until reload completes.
4. Register at least two independent Russian probe networks. Refresh address
   checks and wait for two successful rounds on each prepared reserve.
5. Preview a manual IP/DNS change. Apply it and inspect rotation history through
   DNS verification and old-listener retirement.
6. Enable **auto rotation** last. Disable it from the same pool row whenever
   needed. A started DNS request still needs reconciliation after disabling.

Preparation announces the group domain in proxy links while preserving user
keys and listener transport policy. A switch changes one registered address
family at a time; unknown state in another enrolled family stops automation.
Reserve order follows pool creation order. A retired address is not automatically
rebound or returned to service: prepare the pool again and require six healthy
rounds before reusing a quarantined address.

When a write is uncertain, use **Reconcile uncertain rotation** first. If DNS is
verified but a revision conflict prevented old-listener retirement, preview and
apply a fresh listener repair. For an external record conflict, read current DNS,
acknowledge the exact displayed state, and submit a fresh manual preview. Pending
node edits are rejected while a rotation needs reconciliation. Removing pool
enrollment preserves the current DNS and listener configuration.

| Rotation symptom | First cause to check |
| --- | --- |
| Address missing/unusable | It is not assigned, its interface is down, or it is loopback/link-local |
| Preparation rejected | Wildcard bind, SYN limiting, changed revision, or saved/active mismatch |
| Reserve has no quorum | Firewall/routing, actual FakeTLS/MTProto failure, agent/reference availability |
| Rotation remains queued | Configuration operation, compatibility drift, cooldown or changed quorum |
| DNS changed, old listener remains | Retirement conflict/failure; preview a fresh listener repair |
| Automation stops after editing keys | Verify the new compatibility baseline before enabling it again |


## Tuning the contention shaper

Use the Shaping page to set measured upload/download capacities and optional
`bulk_up_bps` / `bulk_down_bps`. Zero derives the bulk cap from the priority
percentage. A nonzero cap limits the entire bulk class while small traffic is
waiting; spare priority capacity is loaned back as soon as requests are served.
Changes use the displayed revision and apply locally without restarting the node.
Start with automation disabled and adjust the cap from throughput, latency and
window-volume measurements. Volume thresholds are heuristics over encrypted
flows, not a classifier for message contents.

An uncertain pool DNS change has its own **Preview pending DNS repair** and
**Queue displayed DNS repair** actions. Inspect the records first; a complete
accepted write should use reconciliation. A partial write can be retried through
the displayed repair while retaining the rotation state and keeping automation
off. Ordinary DNS-group repair cannot bypass pool ownership.

Local results and remaining production acceptance gates are in [VERIFICATION.md](VERIFICATION.md).
