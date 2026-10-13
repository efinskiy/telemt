# Fleet administration, adaptive shaping and assigned-IP rotation

The agreed target is a separate Rust/React/PostgreSQL control service, a
process-owned IP shaper, authenticated external MTProxy probes, conservative
Cloudflare DNS failover, and rotation among addresses already assigned to node
interfaces. Production servers and DNS are not modified during development.

## Implementation

- [x] Establish baseline build and test results; record existing platform failures separately.
- [x] Add invariant tests before the adaptive limiter implementation.
- [x] Add validated, backward-compatible shaping configuration and runtime API.
- [x] Implement process-owned, bounded IP/flow accounting and directional budgets.
- [x] Integrate cancellation-safe shaping in Direct, Middle and authenticated WEB traffic.
- [x] Add PostgreSQL migrations, encrypted credentials, Argon2/TOTP authentication and RBAC.
- [x] Implement fleet configuration, users, operations, monitoring, retention and audit.
- [x] Implement bounded authenticated FakeTLS/MTProxy probes and result ingestion.
- [x] Implement quorum, compatible failover groups and Cloudflare reconciliation.
- [x] Build the React administration UI and browser scenarios.
- [x] Add source-built image definitions, Compose profiles, backups, restore and rollback instructions.
- [x] Discover assigned interface addresses and create owner-managed IP pools with automation off.
- [x] Prepare reserve listeners, verify external probes, rotate enrolled DNS records and retire old acceptors.
- [x] Fence ordinary writes during rotation; reconcile uncertain results and provide owner-reviewed DNS/listener repair.
- [x] Add configurable directional bulk caps, priority demand reclamation and idle capacity borrowing.
- [x] Preserve calculated IP/class shares under unrelated IP churn and refund every reservation scope.
- [x] Test low-capacity priority, partial I/O, cancellation, live reload, reconnect and per-direction isolation.
- [x] Use read-only subagent reviews and address reported shaping and rotation defects.

## Public management endpoints

- [x] Allow owner-enrolled public/private IP and DNS HTTP/HTTPS origins without requiring VPN.
- [x] Preserve endpoint validation, encrypted tokens, TLS verification and redirect refusal.
- [x] Update node enrollment UI and deployment/API documentation.
- [x] Verify URL boundaries, actual enrollment/editing and hostname requests, browser submission and builds.

## Verification and release gates

- [x] Rust 1.88 Linux all-target checks and release build; standalone backend/agent checks.
- [x] Backend strict lint and PostgreSQL integration covering auth, roles, revisions, probes and DNS recovery.
- [x] Frontend production build, type checking, unit tests and browser scenarios.
- [x] Execute production limiter harness, including 10,000 logical flows and disabled loopback comparison.
- [x] Run protocol/core tests in the temporary macOS portability harness and reproduce failures on baseline.
- [x] Record verification evidence, commands, environment limits and deployment prerequisites.
- [ ] Run the entire unmodified core test suite on a Linux execution host; CI job is provided.
- [ ] Build and start Compose images on a Docker Engine host; local Engine is unavailable.
- [ ] Run 10,000 real Direct/Middle/WEB connections with simultaneous small/bulk traffic and slow peers.
- [ ] Measure full-server disabled-shaper throughput against baseline on an isolated reproducible load host.
- [ ] Exercise real Cloudflare test-zone failure/recovery using two independent Russian probe networks.
- [ ] Perform staged deployment: one node with both automations off, then shaping, reserves/probes, then DNS automation.

## Acceptance constraints

- Static user/CIDR ceilings, KDF, transport framing and fallback behavior remain intact.
- Enabled shaping runs locally without the controller; restart alone resets minute history.
- Every connection of an IP shares its budget, rolling history and priority/bulk scopes.
- The bulk cap applies under outstanding priority demand; spare capacity is borrowable.
- Unused reservations never count as transferred bytes; successful I/O alone is charged.
- UI writes retain their displayed revision and expose partial/unknown operation outcomes.
- Unknown probe state cannot authorize DNS writes; every DNS write is verified and audited.
- A retired pool address requires explicit listener preparation before reuse; no automatic failback.
- Production targets remain 20 nodes, 10,000 connections/node, small-flow p95 extra latency
  <=500 ms below priority demand, and <=3% full-server throughput regression with shaping off.

See `deploy/fleet/VERIFICATION.md` for results and outstanding acceptance evidence.
