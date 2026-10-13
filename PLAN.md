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

## Russian fleet interface and observability

- [x] Translate navigation, forms, status/error messages, dialogs and accessibility labels into Russian.
- [x] Replace configuration/DNS JSON editors with described typed fields and sparse revision-fenced patches.
- [x] Add timestamped traffic, connected-account and connection charts with explicit missing/stale data.
- [x] Extend shaping snapshots with bounded per-account live-flow measurements without changing packet-path scheduling.
- [x] Display account traffic, waiting users/flows, wait duration and shared IP allocations on the shaping page.
- [x] Replace diagnostic and operation JSON dumps with structured cards, facts and tables.
- [x] Verify telemetry invariants, form patches, localization, production builds and browser workflows.

- Evidence: frontend build/typecheck; 22 unit tests; 16 Chromium scenarios; Rust 1.88 Linux all-target check (existing warnings only); 56 limiter tests; 14 backend tests and strict Clippy. New UI snapshots inspected at desktop/mobile sizes.
- Throughput gate remains open: disabled-loopback regression was 17.119% then 3.226% on this shared host. The 10,000-flow small-stream harness passed at p95 165 ms; real network acceptance tests below remain pending.
- No production update or DNS change. Update Fleet and node images to expose the new account counters; old nodes/history explicitly show unavailable fields.

## Live connection map

- [x] Add a bounded live admission snapshot independent of shaping and optional telemetry.
- [x] Resolve registered node and client IPs locally with a city MMDB database; never send client addresses to a geolocation service.
- [x] Add globe/flat map, rotation/pan/zoom, route selection and live connection details to the inspected node overview.
- [x] Distinguish private/unknown locations, truncated observations, stale data and unavailable database/node APIs.
- [x] Ship local map assets, attribution, bounded database installer, Compose mount and deployment instructions.
- [x] Verify snapshot/lookup invariants, map interactions, production builds and Russian mobile presentation.

- Evidence: Rust 1.88 Linux all-target check (existing warnings only); 31 production IP-tracker harness tests; backend release build and strict Clippy; 19 backend unit tests, PostgreSQL router integration (including owner/operator/viewer map access), and a real DB-IP IPv4/IPv6 lookup test. Frontend production build/typecheck, 27 unit tests and 23 distinct Chromium scenarios passed, including 10,000-IP map bounds, stale snapshots, node changes, no external requests, mobile, drag/keyboard/fullscreen and reduced motion. Three offline installer tests and Compose config validation passed.
- Downloaded the real October 2026 DB-IP City Lite database into a temporary local directory and verified it with the production lookup code. Database and browser assets are not fetched on behalf of client IPs. The optional database installer, restart requirement and attribution are documented in deploy/fleet/README.md.
- Desktop globe/flat map and mobile screenshots inspected. Temporary PostgreSQL was stopped after verification. No production deployment, node configuration or DNS changes; Docker image startup remains unverified without a local Engine.
- Also corrected the existing shaping UI's read-only `/v1/fleet/traffic` allowlist omission, with a regression test.

## Zashboard visual redesign

- [x] Apply a shared neutral light/dark/system theme, compact cards, tables and described forms across Fleet.
- [x] Add an icon sidebar with collapse preference and a mobile bottom dock with access to every permitted section.
- [x] Align connection map, charts, status badges, dialogs and login with the shared design tokens.
- [x] Run only the frontend production build/typecheck and essential desktop/mobile smoke scenarios; inspect screenshots.

- Reference: current zashboard light/dark palettes and compact navigation/card patterns inspected at e5b48b4e8fe5c727170a57444c969f01aca3fbad. Fleet uses original React components and shared CSS variables; no framework or backend changes.
- Evidence: frontend production build and strict TypeScript check passed. Exactly four essential browser scenarios ran: desktop section navigation, configuration draft/conflict handling, theme/sidebar persistence, and mobile dock/dialog navigation. Light/dark overview, shaping, mobile diagnostics and bottom-sheet screenshots inspected. No backend, protocol, load or broad regression suites run for this visual-only request.
- Theme follows the system by default; explicit choice and desktop sidebar collapse are persisted locally. All allowed pages remain reachable through the desktop rail or mobile More sheet. Changes are local and not deployed.

## Current verification

- [x] Reproduce HTML responses at the Fleet browser API boundary and replace raw JSON parser failures with actionable routing diagnostics; keep upstream bodies out of error messages.
- [x] Verify response handling (14 frontend unit tests), production build/type checking, and 10 browser scenarios including Users recovery after an HTML 502.
- [ ] Resolve the deployed node-read failure: browser evidence shows Cloudflare HTML 502 on Fleet node reads while local Fleet lists succeed; awaiting domain/direct-IP Control API checks from the controller host.

- [x] Expose Rust image build job counts through `CARGO_BUILD_JOBS`, defaulting to builder-visible CPUs without changing release optimizations.
- [x] Verify all three build commands with 12 shell-harness cases (unset/empty automatic CPU selection and explicit 1/16 jobs), shell syntax and diff checks. Full image compilation remains unexecuted because the local Docker Engine is unavailable.

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
