# Shared profiles and managed nodes

Managed nodes run telemt, nginx and an outbound HTTPS supervisor in one container.
Fleet owns versioned profiles, shared users, certificate material and rollout state.
Existing nodes connected through a Control API continue to work independently.
No Docker socket, SSH access or public node Control API is required.

## Prepare Fleet

1. Back up PostgreSQL and the existing Fleet master key using `backup.sh` before upgrading.
   Keep the master key separately: the database alone cannot recover encrypted credentials.
2. Build or pull the updated Fleet image, including migration `003_managed_profiles.sql`.
   Migrations run during Fleet startup. Do not replace an existing `.env` with the example.
3. Publish the `managed-node` image using the Fleet images workflow. It builds Linux
   amd64 and arm64 images from this repository. Resolve its immutable manifest digest:

   ```sh
   docker buildx imagetools inspect ghcr.io/efinskiy/telemt-managed-node:sha-YOUR_COMMIT_SHA
   ```

4. Add the resulting complete image reference to `deploy/fleet/.env`:

   ```dotenv
   MANAGED_NODE_IMAGE=ghcr.io/efinskiy/telemt-managed-node@sha256:YOUR_PUBLISHED_DIGEST
   ```

   The wizard rejects mutable tags. Recreate the Fleet admin service with your existing
   local-build or registry Compose invocation. This variable affects newly generated files;
   it does not silently replace running node binaries.
5. Fleet's HTTPS reverse proxy must forward `/node-agent/v1/` to the backend without a
   browser login challenge. The supplied nginx configuration already forwards these paths.
   Agent requests authenticate using individual credentials. Keep Fleet's HTTPS domain
   independent of the proxy domain and its failover DNS record.

## Certificate ownership and renewal

In **Сертификаты**, create a certificate entry for the shared node domain, then import
`fullchain.pem` and `privkey.pem`, or enable **Автопродление** to issue one centrally.
Import verifies the chain, SAN, validity and correspondence of the private key.
Private keys and Cloudflare credentials are encrypted in PostgreSQL with Fleet's master key.
They are never returned by certificate listing endpoints.

For central issuance/renewal, provide an ACME contact email, the Cloudflare zone ID and an
API token with DNS Edit permission for that zone. Enabling issuance accepts the ACME
subscriber agreement through the configured Let's Encrypt account. Fleet creates only the
`_acme-challenge.<domain>` TXT value needed by its current order and removes that exact
value after completion. No node needs port 80, certbot or a Cloudflare token. Domain A/AAAA
records may point at any node: DNS-01 does not require the original node to be reachable.
The domain must use the specified Cloudflare zone for authoritative challenge DNS; delegated
challenge CNAMEs and external DNS providers are not supported by this issuer.

Fleet persists the ACME account, order, CSR and private key before finalization. A restart
resumes the same order. ARI schedules renewal inside the CA's suggested window and respects
its retry interval; without ARI the fallback is two thirds of the certificate lifetime.
Errors retain the previous certificate and expose an error with a retry time. If a token
expires during an order, replace the token with the same zone and ACME environment; the
pending order keeps its original account and receives the new DNS credential.

A renewed/imported certificate creates a new deployment for every node. Each supervisor
validates it, reloads nginx and checks the certificate actually served on its public TLS
socket before acknowledging it. Certificate-only changes do not restart telemt. Offline
nodes keep their last certificate and obtain the latest one when they reconnect. They cannot
renew while Fleet is offline; restore Fleet before certificate expiry. Revoking a node's
Fleet credential does not erase previously delivered certificate keys; rotate shared keys
and replace/revoke certificates separately if the node was compromised.

The staging switch is only for isolated test environments with an explicitly configured
`MANAGED_CA_FILE` containing the appropriate test CA on Fleet and all test nodes. Staging
certificates are not trusted by ordinary clients. Do not enable test trust in production.

## Create a shared profile

In **Общие профили**, create a profile linked to the domain's certificate. Add users or
import an existing `telemt.toml`. Import preserves user keys and limits in a draft; it does
not change the source node. Review the form and publish it.

- WEB listens through nginx on HTTPS 443. Native FakeTLS MTProxy defaults to 8443.
- nginx forwards to telemt's WEB listener on loopback 18080. The Control API is on loopback
  19091. These local ports must be free on the host.
- The renderer sets the exact node IP:443 WEB endpoint, while users, secrets, domain and
  WEB base path remain common. KDF and transport implementation are unchanged.
- The assigned IP must exist on a host interface. The wizard does not provision IPs or NAT.
- User quotas, rate limits, shaping state and accounting remain local to each node.
- Per-node upload/download capacities can override profile capacities through **Управление**.
- Saving edits changes the draft. Publishing creates immutable versions and individual
  deployments. Offline/failed nodes remain visibly pending/failed; they are not counted as updated.
- Publishing configuration changes restarts telemt and can disconnect existing sessions.
  Certificate-only renewal is handled by nginx reload. This release does not migrate active
  TCP or WEB sessions between servers; clients reconnect using their existing domain and keys.
- Profile history can roll back configuration by publishing the selected old version as a new
  version. The current certificate remains current; rollback does not restore an expired certificate.

Profile writes require the displayed `If-Match` revision. A concurrent edit returns a conflict
and never overwrites the other administrator's changes. Owners manage enrollment, certificates,
ports and DNS association. Operators can edit users/settings and publish versions. Viewers see
status without private keys. User connection links require an explicit operator action.

## Add a server

1. On a Linux amd64/arm64 host install Docker Engine with the Compose plugin. Open inbound
   443 and the profile's native MTProxy port; allow outbound HTTPS to Fleet and Telegram
   upstream connectivity. A proxy DNS record must be DNS-only, not Cloudflare HTTP-proxied.
2. In **Серверы → Добавить сервер**, enter a name, the assigned IP and a published profile.
3. Copy the generated file to `compose.yml` on that host, protect it and start it:

   ```sh
   chmod 600 compose.yml
   docker compose pull
   docker compose up -d
   docker compose ps
   docker compose logs --tail=100 node
   ```

4. Wait for the desired and applied deployment versions to match in Fleet. No manual nginx,
   telemt configuration or certificate copying is required. Registration expires after 24 hours
   and is consumed on first use. Before first registration, **Управление → Новый файл подключения**
   invalidates the previous registration token. Persist the `state` volume across updates.

The generated production file uses Linux host networking and one node per host. The container
has a read-only root filesystem, bounded logs and a private persistent state directory. The
nginx master retains the bind, UID/GID, ownership and child-signal capabilities needed to
manage its workers; no host PID namespace is shared. The state volume contains plaintext runtime keys required to serve TLS; protect host backups and
root access. The agent validates configurations before activation, preserves a last-good bundle,
rolls back failed activation and starts from committed state if Fleet is unreachable. Local edits
are reported as drift. Use **Повторить применение** after correcting an external activation failure.

For a binary update, change the image digest in the node's existing Compose file, pull and
recreate the service. Keep its `state` volume and node identity. Do not delete volumes with
`down -v` during an update. Roll back binaries with the prior digest and the same volume.
For Fleet binary/schema rollback, use the pre-upgrade database backup plus its matching master
key and image; older binaries must not be assumed compatible with new migrations.

## DNS groups and migration

Create the existing Cloudflare DNS failover group for the shared domain and verified active
node, then associate the profile with it. A new node is admitted as a reserve only after it
acknowledges the desired profile/certificate, passes readiness and obtains native **and WEB**
successes from the required independent external probe networks. Upgrade probe agents too.
Automatic failover retains the existing quorum, cooldown and DNS readback rules. Conflicting
external DNS changes and unknown writes require reconciliation; profile publication does not
clear those conditions. A profile port change requires detaching/reconfiguring its DNS group.

To adopt a direct-API node, import its TOML into the profile, preserve all user keys, publish
and choose **Перевести на профиль**. The node must run a version exposing the user compatibility
fingerprint. Fleet rejects mismatching keys and domains. Stop the old telemt/nginx deployment
before starting the generated managed file, since ports must not overlap. Keep the old config,
certificate and Compose file for rollback. Migration may change the native port; verify links.
After rollback to the old deployment, reconnect it as a direct-API node in Fleet.

Single-server IP-pool rotation currently belongs to the direct-API deployment path. A managed
profile uses its assigned node IP and DNS switching between servers. Remove a direct node's
IP pool before adoption; creation of an IP pool for a managed node is explicitly rejected.
No listener or DNS mutation is silently performed during adoption or enrollment.

## Local verification

The test harness uses real binaries, nginx and PostgreSQL with temporary local certificates.
It does not call Cloudflare or change public DNS. It creates two isolated bridge-network nodes
instead of production host networking and deletes only its own containers/volumes on exit.
Prerequisites: Docker/Compose, Python 3.11+, OpenSSL, free local port 18444 and subnet
172.30.91.0/24. Override the latter with `--port` and `--subnet`.

```sh
docker build -f deploy/fleet/Dockerfile --target admin -t telemt-admin:managed-test .
docker build -f deploy/fleet/Dockerfile --target managed-node -t telemt-managed-node:local .
python3 deploy/fleet/tests/managed_smoke.py
```

It checks enrollment, revisions, actual node readiness, mismatched certificate rejection,
certificate replacement without telemt restart, two-node user rollout, rollback preserving
client links, local drift repair, and restart from committed state with Fleet stopped. `--keep` retains the isolated
fixtures for diagnosis; they contain test-only keys and registration tokens. Production image
builds and service startup do not run Rust tests. CI runs tests separately.

A separate opt-in real Telegram handshake check is available in
`agent::web::tests::live_managed_web_round_trip`; it requires a test job file and explicit test
CA trust. Live ACME/Cloudflare issuance and two-network failover require a disposable real DNS
zone and registered external agents and are not exercised by this local harness.
