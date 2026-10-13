# Fleet implementation file index

This index describes the current working-tree patch. Build and verification status
are recorded separately in `../deploy/fleet/VERIFICATION.md`.

`.dockerignore`
Exclude local artifacts and secret directories from source image build contexts.

`.github/workflows/fleet.yml`
Run MSRV/stable, PostgreSQL, frontend/browser and source-image checks.

`.gitignore`
Exclude generated frontend, Rust, browser and local secret artifacts.

`PLAN.md`
Track implementation completion and outstanding external acceptance gates.

`admin/backend/Cargo.lock`
Lock reproducible Rust dependency resolution for this package.

`admin/backend/Cargo.toml`
Define this independently buildable Rust package and its required dependencies/MSRV.

`admin/backend/migrations/001_initial.sql`
Create central identity, inventory, session, operation, monitoring, probe and DNS tables.

`admin/backend/migrations/002_ip_rotation.sql`
Add bounded assigned-IP pools, address-specific probes and durable rotation state.

`admin/backend/src/auth.rs`
Implement password/TOTP login, server sessions, CSRF, roles and administrator lifecycle.

`admin/backend/src/crypto.rs`
Provide Argon2id, TOTP and resource-bound authenticated secret encryption.

`admin/backend/src/dns.rs`
Expose enrolled DNS groups, readiness and conservative failover coordination.

`admin/backend/src/dns/management.rs`
Manage group membership, credentials, compatibility checks and automation settings.

`admin/backend/src/dns/provider.rs`
Fence Cloudflare writes, commit intents, verify results and reconcile uncertain outcomes.

`admin/backend/src/error.rs`
Return bounded structured API errors without exposing secret details.

`admin/backend/src/integration_tests.rs`
Exercise the actual router and PostgreSQL against bounded local node/provider fixtures.

`admin/backend/src/integration_tests/node_api_urls.rs`
Verify owner-only public endpoint enrollment/editing, retained credentials and authenticated hostname requests.

`admin/backend/src/integration_tests/rotation_cases.rs`
Verify pool enrollment, authorization, conflicts, partial DNS repair, retirement and family filtering.

`admin/backend/src/main.rs`
Start the central service, migrate the database, run bounded background workers and expose bootstrap CLI commands.

`admin/backend/src/monitor.rs`
Collect restart-aware metrics and maintain detailed/aggregate/audit retention.

`admin/backend/src/operations.rs`
Persist per-node operation progress, revision checks, reload tracking and uncertain-result recovery.

`admin/backend/src/probes.rs`
Schedule authenticated one-use probe jobs and evaluate independent-network health quorum.

`admin/backend/src/rotation.rs`
Expose assigned-IP pools, manual/automatic control, previews and operation history.

`admin/backend/src/rotation/policy.rs`
Validate assigned addresses, prepare listener changes and maintain address-scoped health.

`admin/backend/src/rotation/recovery.rs`
Repair uncertain listener/DNS outcomes from owner-reviewed fresh snapshots.

`admin/backend/src/rotation/workflow.rs`
Coordinate listener preparation, DNS rotation, old listener retirement and automation.

`admin/backend/src/routes.rs`
Dispatch authorized fleet requests with constrained node API paths.

`admin/backend/src/state.rs`
Centralize bounded HTTP node requests, configuration, audit and database state.

`admin/frontend/index.html`
Configure the frontend entry point, strict type checking or browser/unit-test runtime.

`admin/frontend/package-lock.json`
Lock frontend dependencies for reproducible production builds.

`admin/frontend/package.json`
Define frontend dependencies and build/test commands.

`admin/frontend/playwright.config.ts`
Configure the frontend entry point, strict type checking or browser/unit-test runtime.

`admin/frontend/src/api.test.ts`
Test API errors, CSRF and stale/partial response handling.

`admin/frontend/src/api.ts`
Centralize typed requests, CSRF and revision-aware node access.

`admin/frontend/src/components.tsx`
Provide shared forms, tables, dialogs and status rendering.

`admin/frontend/src/fleet-pages.tsx`
Manage fleet enrollment, agents, DNS groups, operations, audit and administrators.

`admin/frontend/src/main.tsx`
Render authenticated navigation and role-aware administration pages.

`admin/frontend/src/node-pages.tsx`
Manage users/configuration, configurable class caps, diagnostics and WEB sessions.

`admin/frontend/src/rotation-page.tsx`
Build assigned-address pools and expose preparation, rotation, automation and repair controls.

`admin/frontend/src/styles.css`
Style responsive desktop/mobile administration views.

`admin/frontend/tests/console.spec.ts`
Verify navigation, roles, revisions, WEB commands, IP pools and bulk-cap saves in the browser.

`admin/frontend/tsconfig.json`
Configure the frontend entry point, strict type checking or browser/unit-test runtime.

`admin/frontend/vite.config.ts`
Configure the frontend entry point, strict type checking or browser/unit-test runtime.

`admin/frontend/vitest.config.ts`
Configure the frontend entry point, strict type checking or browser/unit-test runtime.

`agent/Cargo.lock`
Lock reproducible Rust dependency resolution for this package.

`agent/Cargo.toml`
Define this independently buildable Rust package and its required dependencies/MSRV.

`agent/src/main.rs`
Poll authenticated bounded probe jobs, check reference connectivity and submit one-use results.

`agent/src/protocol.rs`
Perform FakeTLS/MTProxy authentication and validate req_pq_multi/resPQ nonce within bounded deadlines.

`agent/src/protocol/wire_tests.rs`
Exercise protocol parsing/authentication bounds and optional real-node round trips.

`deploy/fleet/.env.example`
List deployment environment inputs without real secrets.

`deploy/fleet/Dockerfile`
Build node, backend/frontend and probe images from the locally changed source.

`deploy/fleet/README.md`
Provide installation, secrets, HTTPS, backup/restore, rollback and staged rollout procedures.

`deploy/fleet/VERIFICATION.md`
Record executed checks, measurements, baseline failures and remaining release gates.

`deploy/fleet/backup.sh`
Back up the central database through the deployed PostgreSQL service.

`deploy/fleet/compose.http.yml`
Provide the initial HTTP certificate-bootstrap override.

`deploy/fleet/compose.yml`
Compose PostgreSQL, central admin, HTTPS proxy and optional node/probe services.

`deploy/fleet/nginx.http.conf.template`
Serve certificate challenges before HTTPS activation.

`deploy/fleet/nginx.https.conf.template`
Proxy the cabinet over HTTPS and constrain request handling.

`deploy/fleet/renew.service`
Define the deployment certificate-renewal service.

`deploy/fleet/renew.sh`
Renew certificates and reload the reverse proxy after success.

`deploy/fleet/renew.timer`
Schedule certificate renewal on the deployment host.

`deploy/fleet/restore.sh`
Restore a selected database backup with explicit operational checks.

`docs/Fleet-changes.md`
Index every file in this implementation patch.

`docs/Fleet-control.md`
Describe architecture, contracts, shaping, probes, failover and assigned-IP rotation.

`src/api/config_store.rs`
Allow validated shaping updates through the existing configuration API.

`src/api/handler.rs`
Register interface inventory support in the existing API handler module.

`src/api/handler/interface_inventory.rs`
Read assigned usable interface addresses and active listener state with bounded output.

`src/api/handler/read_routes.rs`
Expose bounded shaping state, fleet compatibility/counters and interface inventory.

`src/config/hot_reload/fields.rs`
Apply shaping policy through the existing hot overlay.

`src/config/load.rs`
Validate optional shaping policy during configuration loading.

`src/config/load/strict_keys.rs`
Recognize the shaping section in strict configuration checking.

`src/config/load/validate_core.rs`
Include shaping validation in the existing core validation pipeline.

`src/config/types.rs`
Include the optional shaping policy in serialized proxy configuration.

`src/config/types/shaping.rs`
Define and validate capacities, contention bulk caps, priority share, thresholds and state bounds.

`src/maestro/generation.rs`
Initialize shared transport lease dependencies for authenticated clients.

`src/maestro/orchestrator.rs`
Publish initial process-local shaping policy.

`src/maestro/reload_supervisor.rs`
Publish shaping updates with source-generation fencing.

`src/maestro/runtime_tasks.rs`
Advance shaping calculations from the existing periodic runtime worker.

`src/metrics/render/traffic.rs`
Export aggregate shaping usage, waits, table exhaustion and calculation state.

`src/proxy/authenticated.rs`
Carry the authenticated transport lease into Direct/Middle relay dispatch.

`src/proxy/client/authenticated.rs`
Preserve existing entry points while sharing authenticated transport shaping state.

`src/proxy/client/stream_entry.rs`
Attach shaping to authenticated buffered stream paths and preserve fallback.

`src/proxy/client/tls_client.rs`
Place shaping below authenticated TLS record buffering for actual transport accounting.

`src/proxy/direct_relay.rs`
Expose the Direct relay entry point that accepts a shared shaping lease.

`src/proxy/direct_relay/relay.rs`
Apply shared-IP pacing and distinguish intentional budget waits from idle peers.

`src/proxy/middle_relay.rs`
Expose the Middle relay entry point that accepts a shared shaping lease.

`src/proxy/middle_relay/idle.rs`
Include intentional pacing in Middle relay activity tracking.

`src/proxy/middle_relay/idle/read.rs`
Exclude actual shaping wait time from network idle deadlines.

`src/proxy/middle_relay/quota.rs`
Expose the lease needed for pacing-aware quota/idle handling.

`src/proxy/middle_relay/session.rs`
Integrate a shared lease and pacing-aware activity into the Middle relay session.

`src/proxy/middle_relay/session/entry.rs`
Preserve existing Middle relay signatures through a compatibility entry point.

`src/proxy/relay.rs`
Carry the pacing-aware relay wrapper through generic forwarding.

`src/proxy/relay/adaptive_copy.rs`
Avoid interpreting intentional pacing as a stalled relay.

`src/proxy/traffic_limiter.rs`
Own process-local adaptive state alongside existing static limit policy.

`src/proxy/traffic_limiter/adaptive.rs`
Maintain IP/flow history and enforce global, IP and class budgets with refundable reservations.

`src/proxy/traffic_limiter/adaptive/class_tests.rs`
Test reclaim/borrowing, class fairness, churn, refunds, tiny capacities and directional reload.

`src/proxy/traffic_limiter/adaptive/control.rs`
Calculate rolling volumes, hysteresis and weighted max-min class allocations.

`src/proxy/traffic_limiter/adaptive/io.rs`
Pace actual reads/writes with cancellation-safe timers and partial-operation accounting.

`src/proxy/traffic_limiter/adaptive/regression_tests.rs`
Test reconnect, idle/churn behavior, disabled telemetry/reload and 10,000 logical flows.

`src/proxy/traffic_limiter/adaptive_tests.rs`
Test core adaptive state, window, budget, cancellation and admission invariants.

`src/proxy/traffic_limiter/buckets.rs`
Reuse existing atomic budget accounting with extracted IP admission support.

`src/proxy/traffic_limiter/buckets/active.rs`
Implement bounded epoch admission and frozen weighted IP credit.

`src/proxy/traffic_limiter/limiter.rs`
Attach bounded adaptive IP/flow admission to existing leases.

`src/stream/crypto_stream.rs`
Map inner transports while preserving cryptographic stream state.

`src/stream/tls_stream.rs`
Register transport mapping support for buffered TLS streams.

`src/stream/tls_stream/transport_map.rs`
Preserve buffered TLS state when attaching/removing transport pacing and test physical byte accounting.

`tools/shaping-harness/Cargo.lock`
Lock reproducible Rust dependency resolution for this package.

`tools/shaping-harness/Cargo.toml`
Define this independently buildable Rust package and its required dependencies/MSRV.

`tools/shaping-harness/src/config/mod.rs`
Provide the minimal configuration contracts needed by the production limiter.

`tools/shaping-harness/src/disabled_benchmark.rs`
Compare disabled production-wrapper and baseline loopback throughput.

`tools/shaping-harness/src/lib.rs`
Load the actual production limiter for portable invariant tests.

`tools/shaping-harness/src/traffic_limiter`
Symlink the production limiter submodules rather than maintain a test-only implementation.

`tools/shaping-harness/src/traffic_limiter.rs`
Symlink the production limiter entry module.
