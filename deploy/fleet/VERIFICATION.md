# Verification record

Date: 2026-10-13. Development host: Apple Silicon macOS. These are local
implementation results, not a production deployment certificate. No production
servers or Cloudflare records were changed.

## Verified

| Component | Result | Scope |
| --- | --- | --- |
| Rust 1.88 Linux node | All-target check and release build passed | Actual repository, `aarch64-unknown-linux-musl`; Zig linker; Linux executable was not run on macOS |
| Rust 1.88 Linux backend/agent | All-target checks passed | Actual independent manifests and locked dependencies |
| Backend | 13 tests passed | Actual Axum router, disposable PostgreSQL, migrations, roles, CSRF, session revocation, operations and probe/DNS/IP-rotation recovery |
| Backend/agent lint | Strict Clippy passed | Standalone packages, all targets, `-D warnings` |
| Probe agent | 4 passed, 1 ignored | Bounded wire protocol tests; optional real-node test requires a configured endpoint |
| Shaper harness | 56 passed, including both load tests | Symlinks to production limiter/config; Rust 1.88 release, sequential test execution |
| Latest node limiter tests | 54 passed, 1 load test ignored | Latest source in temporary macOS portability harness; heavy test separately passed in production-source harness |
| TLS transport accounting | 2 passed | Actual buffered TLS reader/writer mapping, successful wire bytes, pending writes and disable behavior in portability harness |
| Frontend | Production build/type checking and 3 unit tests passed | Locked React/TypeScript/Vite application |
| Browser | 8 scenarios passed | Chrome, desktop/mobile, roles, revision conflicts, WEB actions, assigned-IP pools and configurable bulk caps; API fixtures |
| Deployment definitions | Compose configuration and shell syntax passed | Both Compose configurations; Engine execution remains pending |
| Mutation smoke test | Removed IP-table bound was detected | Temporary copy failed the admission-exhaustion invariant as expected; production source unchanged |

The latest 10,000-logical-flow run used 9,000 bulk flows and 1,000 small flows:
small-flow p95 completion delay **157 ms**, control calculation **4 ms**. The
sequential disabled-wrapper loopback comparison transferred 2 GiB per run, used
warm-up plus seven alternating samples, and measured median baseline **0.155484 s**
and disabled **0.156548 s**, a **0.679% throughput regression**. A concurrent-load
attempt measured **5.226%** and failed the unchanged 3% assertion; the sequential
rerun passed. This host-sensitive microbenchmark does not establish the full-server
3% acceptance target. An earlier 10,000-flow memory run peaked at 108,806,144 bytes;
that predates the added pending-class atomics and is not the final memory bound.

The load test uses real production reservations and accounting but **not 10,000
Direct/Middle/WEB TCP sessions**. It must not be cited as full relay/load acceptance.
One earlier read-only real FakeTLS/MTProxy `req_pq_multi`/`resPQ` exchange through a
temporary Direct node succeeded. No equivalent real Middle/WEB load claim is made.

## Existing core limitations

The complete temporary macOS portability-harness run, before the final class-budget
extension, reported **2525 passed, 5 failed, 71 ignored**. Each failing case was
reproduced against the baseline using the same temporary portability adjustments:

- `synlimit_synfix_defaults_are_loaded_for_listener`
- `unknown_dc_log_path_sanitizer_accepts_symlink_parent_escape_as_canonical_path`
- `subtle_integration_parallel_unique_dcs_log_unique_lines`
- `bound_refresh_survives_cancellation_and_reuses_only_its_source`
- `delayed_valid_handshake_reaches_the_authenticated_relay`

The actual root package does not compile natively on this macOS host because of
pre-existing `tcp_mss_runtime_profile` platform gating and `mode_t` u16/u32 type
mismatches. Temporary copies adapted those platform-only boundaries for test
execution; those adaptations are not included in the delivered patch. The actual
Linux release build succeeds and retains 47 existing warnings. Existing tests
and assertions were not weakened. The complete unmodified suite must still run
on Linux; the added CI workflow runs it and does not ignore its failures.

## Reproduce on Linux

Use a disposable PostgreSQL database for the integration test. The test creates
an isolated schema and removes it after success. Keep real DNS credentials out
of the test environment; the test provider is local.

```sh
cargo +1.88.0 check --all-targets --locked
cargo +1.88.0 build --release --locked
cargo +1.88.0 test --locked
cargo +1.88.0 clippy --all-targets --locked
cargo +1.88.0 test --manifest-path admin/backend/Cargo.toml --locked -- --include-ignored
cargo +1.88.0 clippy --manifest-path admin/backend/Cargo.toml --all-targets --locked -- -D warnings
cargo +1.88.0 test --manifest-path agent/Cargo.toml --locked
cargo +1.88.0 clippy --manifest-path agent/Cargo.toml --all-targets --locked -- -D warnings
cargo +1.88.0 test --release --manifest-path tools/shaping-harness/Cargo.toml --locked -- --include-ignored --nocapture --test-threads=1
npm ci --prefix admin/frontend
npm run build --prefix admin/frontend
npm test --prefix admin/frontend
npm exec --prefix admin/frontend -- playwright install --with-deps chromium
npm run test:browser --prefix admin/frontend
```

Set `TELEMT_TEST_DATABASE_URL` to the disposable database before running backend
tests. On Linux, install the system dependencies required by the existing root
package and browser before testing. The source-built image definitions and CI
workflow contain those setup steps.

## Remaining acceptance gates

- A Docker Engine host must build and start the node/admin/probe images and run
  backup/restore, certificate renewal and staged rollback checks. Docker Engine
  is unavailable on the development host; configuration validation is not runtime
  validation.
- An isolated Linux load host must run 10,000 real Direct/Middle/WEB connections,
  slow relay cancellation, IP churn, memory bounds and baseline throughput.
- A Cloudflare test zone, explicitly enrolled record IDs/token, assigned node IPs,
  domains/certificates and two independently registered Russian probe networks
  are needed for actual external failover acceptance. Local provider simulations
  cover stale quorum, partial writes, conflicts, unavailable reserves and recovery.
- Tune measured directional capacity, small-flow thresholds and contention bulk
  caps before enabling the local shaper. A pool address retired after rotation
  requires explicit preparation before reuse; exhausted pools stop with a reason.
- Follow the staged rollout in `README.md`: one node with shaping/automation off,
  then shaping, then reserves/probes, then DNS automation.


## Public management endpoint update

The VPN/private-literal restriction was removed after the initial implementation.
Owner enrollment/editing now accepts public/private IPv4, IPv6 and DNS names for
HTTP/HTTPS API origins. TLS verification, redirect refusal, token encryption and
role checks are unchanged. The added acceptance test first failed against the
private-only validator and passed after the change.

Validation: Rust 1.88 backend tests **15 passed**, including actual PostgreSQL
registration/editing and an authenticated hostname request resolved to the local
fixture; strict all-target Clippy passed; Linux all-target cross-check passed;
frontend production build/type checking passed; browser suite **9 passed**.
No real node endpoint or deployment was changed.
