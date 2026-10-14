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

## Sustained IP shaping and Linux queue management

- [x] Add tested directional sustained-IP detection (rate, duration, volume and recovery), retaining history across reconnects and reloads.
- [x] Add independent idle bulk ceilings without relaxing global, IP, user or CIDR limits.
- [x] Propagate authenticated DC hints through Direct/Middle/WEB; sustained IP limits override priority hints.
- [x] Ship opt-in Linux HTB/fq_codel management with ownership checks, bounded commands, rollback, systemd deployment and status.
- [x] Expose new settings and IP state in Fleet with descriptions, examples and rollout instructions.
- [x] Run production limiter regressions, Linux build checks, queue-manager tests and essential frontend checks; record unavailable live-kernel validation explicitly.
- [ ] Execute the opt-in real Linux namespace test and measure full-server AQM latency/throughput before production rollout.

- Classification contract: sustained IP load requires rate, consecutive duration and rolling volume together. It defaults to 1 Mbit/s, 10 seconds and 8 MiB when shaping is enabled; `sustained_secs = 0` restores flow-only classification. Recovery defaults to 30 measured seconds below half the threshold without adaptive budget pressure. Same-second reload samples and refusal history survive successful grants/reconnects. Existing flow-volume thresholds remain in effect. Authenticated main-DC priority is opt-in, client-selected and never a message-content guarantee.
- Kernel contract: a separate host service owns one all-egress HTB/fq_codel tree, including upstream and non-Telemt traffic. No IFB ingress changes or NET_ADMIN privilege on the Telemt process. Creation rejects foreign roots, removal checks the saved configuration including class-level filters, and matching state recovers over kernel defaults after reboot. Installation/rollback instructions and tunable examples are in `deploy/shaping/README.md`.
- Evidence: idle ceiling and two pressure-history regressions failed before their fixes. Rust 1.88 production limiter harness: 63 passed; optional 10,000-logical-flow run passed with small-flow p95 157 ms and controller calculation 6 ms (not real relay sockets or a Linux kernel queue). Linux aarch64-musl all-target check and optimized node build passed. Frontend production build/typecheck, 27 unit tests and three essential browser scenarios passed. Python queue lifecycle: nine passed, one real-kernel namespace test skipped on macOS. Existing tests were not weakened. Read-only subagents reviewed protocol integration and real iproute2/kernel output contracts.
- Strict harness Clippy remains blocked by three pre-existing findings: `adaptive/control.rs:46` suspicious_else_formatting and `traffic_limiter/tests.rs:352,366` manual_contains/int_plus_one. The node build reports 47 existing unused/dead-code warnings; these unrelated warnings were not changed. No claim of a warning-free whole-repository build.
- Live Linux namespace/physical queue verification and full relay throughput remain unavailable here: macOS host, no running Docker Engine. No production hosts, DNS, deployment secrets or network policies changed. Update the node before saving new fields from Fleet; older node versions reject unknown shaping fields.

## Current verification

- [x] Reproduce DNS enrollment failures with valid fields and unavailable compatibility, stale monitoring or mismatched addresses.
- [x] Replace generic DNS enrollment errors with specific codes and actionable Russian messages without relaxing enrollment checks.
- [x] Verify backend enrollment regression cases and essential frontend DNS form behavior; no live DNS changes.

- Evidence: the new PostgreSQL router regression reproduced the former generic 400 before the fix; it now verifies separate stale/compatibility/IPv4/IPv6 refusals and successful IPv4-only enrollment without DNS mutation. Rust 1.88 release build and strict all-target Clippy passed. Frontend production build/typecheck and exactly two DNS browser scenarios passed, including field retention after errors. The temporary PostgreSQL instance was stopped. The deployed failure's exact gate remains unknown pending node snapshot diagnostics; no production deployment or DNS changes were performed.

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
- [x] Run the existing core test suite on Linux: 2563 passed, 73 ignored on Rust 1.88 (2026-10-14).
- [x] Build and start local Compose Fleet/PostgreSQL/nginx and two managed nodes on Docker Engine (2026-10-14).
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


## Deployment binary selection

- [x] Inspect node/Fleet startup and image builds: no Rust test invocation exists in the deployment path.
- [x] Select only the named production binaries in all three Rust image stages; preserve parallelism and runtime health checks.
- [x] Verify all three target names through Cargo metadata and 12 build-command cases (automatic/empty/1/16 jobs), without running Rust tests. Full Docker image builds were not executed because the local Docker Engine is unavailable.

## Per-server overview cards

- [x] Add bounded read-only Linux resource telemetry and process/boot-safe monitor deltas; collect existing DC and route status.
- [x] Build Russian per-server cards, current alerts, traffic/shaping/resources/error/DC sections while preserving map/history.
- [x] Add on-demand bounded client IP tables with traffic/geography and explicit unavailable/stale states.
- [x] Run essential frontend scenarios and builds; document measurement scope and limitations.


- Evidence: frontend production build/typecheck and three focused Chromium scenarios passed (rich per-node cards/IP drilldown, stale/missing data, existing mobile history). Desktop and 390 px mobile screenshots inspected in both existing visual layouts, including dark cards and a long IPv6 address. No broad frontend suite was added.
- Backend release build and strict all-target Clippy passed; the focused counter-identity regression passed. A temporary harness executed the actual resource parser test and unsupported-OS response test (two passed). Linux Rust 1.88 all-target check and final aarch64-musl release build passed with the existing 47 node warnings. Real Linux resource collection, Docker deployment and production integration were not executed from this macOS environment.
- The new resource endpoint is read-only and bounded. The data plane, protocol, shaper scheduling, credentials and mutation targets are unchanged. CPU/TCP deltas are reset appropriately across reboot/process changes; null sources remain unavailable. Update node plus Fleet; no database migration is required. Scope and limitations are documented in `docs/Fleet-control.md`.

## Cached GitHub images and CI repair

- [x] Inspect actual GitHub run logs: release/images succeed; formatting, test-only constructor lint and platform-specific screenshot paths fail.
- [x] Fix confirmed CI failures without disabling assertions or checks; formatting explicitly authorized.
- [x] Add cached native amd64/arm64 GHCR builds and commit-addressed images for node, Fleet and probe.
- [x] Add image-only Fleet and standalone-node deployment with upgrade/rollback instructions.
- [x] Validate workflow syntax, Compose resolution, Rust checks and affected browser scenarios; report unavailable live publication.

- Evidence: actionlint passes for all three changed workflows; both registry/standalone Compose configurations resolve with no application build entries. Eleven affected Chromium scenarios and their production frontend build passed; screenshot assertions were not weakened. Rust 1.88 formatting check and Linux all-target Clippy pass (existing warnings remain).
- Pinned cargo-chef 0.1.71 installed successfully on Rust 1.88. All three actual project recipes remain byte-identical after source-only edits and reconstruct locked Cargo metadata. Twenty-four shell cases validate dependency/final binary build commands; six mocked manifest cases cover main/commit tagging, stale reruns and rejection of missing/invalid/extra digest artifacts. No live registry writes were performed by these checks.
- Workflow changes are local until committed and pushed. Native GitHub image builds, GHCR publication and cold/warm timing comparison remain unexecuted here because the Docker Engine is unavailable. Deployment instructions cover package permissions, public/private pulls, pinned tags, existing node mounts and rollback without replacing .env or secrets.
- Final Rust 1.88 aarch64-linux-musl optimized node build passed after the CI fixes; existing node warnings remain. No Rust test execution was added to image builds or service startup.

## GitHub publication verification

- [x] Commit and push the prepared application/deployment changes to main (`574a14c`).
- [x] Confirm GitHub Build, Check and CodeQL succeed; frontend tests succeed and both Fleet Rust jobs reach integration tests.
- [x] Diagnose the remaining CI environment failure: `--include-ignored` enables the real GeoIP test without providing `TELEMT_TEST_GEOIP_DATABASE`. Prepare and cache the real monthly DB-IP database before backend tests; keep test assertions unchanged.
- [x] Confirm the corrected commit passes every workflow and inspect all GHCR manifests for amd64/arm64 and matching main/SHA tags (`b18711dcc8f0ed8b71f195ab48d1f971f51e0223`).

- [x] Fix four strict stable-Clippy nested-condition findings without changing policy; give DNS integration fixtures a fixed probe round and explicitly verify that the next missing round remains unknown. Production quorum and wall-clock freshness checks remain unchanged.
- Local validation of this repair: all 22 backend tests passed with disposable PostgreSQL and the real GeoIP database; Rust 1.88 all-target Clippy passed with warnings denied. Final GitHub workflows and GHCR verification are pending for the repair commit.
- [x] Reproduce and fix the randomized WEB bridge template assertion with an all-underscore valid CSP nonce; retain the template-remnant check, validate nonce length, and execute both POST/PUT browser behavior cases. Replace the probe frame alignment expression with the equivalent MSRV-compatible `is_multiple_of` required by current stable Clippy.
- GitHub at `59a0048`: Build, Check, CodeQL and all image jobs passed; backend tests and strict lint passed on stable. Remaining Fleet failures were the randomized nonce assertion (1.88) and probe alignment lint (stable). All three public GHCR indexes contain amd64/arm64 and matching `main`/commit tags with the expected revision.

- Final application verification: `b18711d` passed Build (38480488409), Check (38480488425), CodeQL (38480488587), Fleet control and shaping (38480488546), and Fleet images (38480488451). Fleet tests and strict Clippy passed on Rust 1.88 and stable; browser tests passed.
- Anonymous GHCR inspection confirmed `telemt-admin`, `telemt-node`, and `telemt-probe` indexes contain Linux amd64/arm64; each image config has the exact application commit revision and `main` matches its full-SHA tag. No production deployment or DNS mutation was performed.

## Managed cluster profiles and one-file node enrollment

- [x] Add versioned shared profiles/users, certificate storage, enrollment identities and durable managed-node commands.
- [x] Implement central certificate import, DNS-01 issuance/renewal and verified bundle distribution.
- [x] Build the autonomous managed-node supervisor, safe configuration validation/activation/recovery and HTTPS management transport.
- [x] Integrate monitoring, DNS admission, WEB compatibility and external WEB probes without changing existing direct-node behavior.
- [x] Add Russian profile/certificate forms, shared users and the name/IP/profile-to-Compose wizard.
- [x] Package native managed-node images, migration/backup/restore documentation and local Compose verification.
- [x] Run focused backend/agent invariants, primary frontend scenarios and real local Docker end-to-end checks.

Decisions: shared users, immediate parallel rollout, local quotas, imported certificates followed by central ACME renewal, WEB on 443 and native MTProxy on configurable 8443, verified automatic reserve admission. Existing installations are imported explicitly. No live production DNS changes.


Verification evidence (local, 2026-10-14):
- Rust 1.88 optimized Linux node/Fleet/supervisor images built; strict backend, supervisor and probe Clippy passed on Rust 1.88 and 1.94.
- PostgreSQL integration and backend unit checks: 24 passed. Shared deployment contract invariant: 1 passed. Frontend: production build/typecheck, 27 unit checks, 31 browser scenarios; 2 focused managed scenarios rerun after UI changes.
- Real Docker two-node lifecycle: enrollment, TLS readback, certificate replacement without telemt restart, shared user rollout, configuration rollback preserving client links, local drift detection/repair, and both nodes restarting successfully while Fleet is offline. Reproducible harness: `deploy/fleet/tests/managed_smoke.py`.
- Real Telegram native FakeTLS and WEB req_pq/resPQ nonce exchange passed against the local managed stack using the opt-in probe test.
- Workflow syntax and root formatting checks passed. Full existing Linux core suite on Rust 1.88: 2563 passed, 0 failed, 73 ignored. The release-profile test link was killed with SIGKILL; the ordinary test profile with two jobs and no debug information passed. Production release images built successfully.
- Live ACME issuance/renewal against a Cloudflare zone and two-network DNS failover are not executed: they require an external disposable DNS zone and probe networks. No production node/DNS changes, commits, pushes or GHCR publication were performed for this implementation.
- Managed profiles support inter-server DNS switching. Existing single-server IP pools remain on direct-API nodes; migration with an attached pool and pool creation on a managed node are rejected explicitly.
