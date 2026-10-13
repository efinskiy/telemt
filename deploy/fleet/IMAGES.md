# Prebuilt Fleet and node images

The `Fleet images` GitHub Actions workflow builds this checkout on native Linux
amd64 and arm64 runners. Every push and manual run builds all three images;
pull requests build without publishing. No production server compiles Rust.

| Service | Image in this repository |
| --- | --- |
| Fleet backend and UI | `ghcr.io/efinskiy/telemt-admin` |
| Node | `ghcr.io/efinskiy/telemt-node` |
| Probe | `ghcr.io/efinskiy/telemt-probe` |

Each successful publication creates `sha-<full 40-character commit SHA>` tags.
The default branch also updates `main`. There is no `latest` tag for these
images. Use the same SHA tag for Fleet and nodes. The original
`ghcr.io/efinskiy/telemt` release image is a separate upstream-compatible image.

## Enable publication

Commit and push the workflow, Dockerfile and application changes together.
Previously pushed commits do not acquire images retroactively. Wait for all six
build jobs and all three manifest jobs in `Fleet images` to succeed before
updating hosts. The run summary gives each published image reference.

Publishing uses the repository's `GITHUB_TOKEN` with `packages: write`; no PAT
belongs in the workflow. Repository/organization policies must permit that
permission. If a package already exists, grant this repository Actions write
access to it. For anonymous pulls, set each GHCR package's visibility to public;
repository visibility alone does not guarantee public packages. Otherwise,
authenticate on the deployment host with `docker login ghcr.io -u USERNAME`
using a personal access token (classic) with `read:packages` when prompted. Do not put it into Compose.

A failed architecture build prevents publication of all three commit manifests.
The three manifest jobs are separate: if publication fails partway through,
wait for a successful retry and pin one SHA on all hosts. Rust tests and browser
checks remain separate CI workflows; a published image is a successful image
build, not a claim that every independent CI check passed.

## Existing Fleet host

Requires Docker Compose 2.24.4 or later. The registry override removes `build`
from each application service; it preserves existing volumes, secrets, networks,
ports and health checks. Back up the database using `backup.sh` before upgrades.
Do not replace an existing `.env` with `.env.example`.

From `/opt/telemt/deploy/fleet`, add these settings to the existing `.env`:

```dotenv
TELEMT_IMAGE_PREFIX=ghcr.io/efinskiy/telemt
TELEMT_IMAGE_TAG=main
```

For a pinned deployment, replace `main` with the `sha-...` value shown in the
successful GitHub workflow. Then run:

```sh
docker compose -f compose.yml -f compose.registry.yml pull admin
docker compose -f compose.yml -f compose.registry.yml up -d --no-build --no-deps admin
docker compose -f compose.yml -f compose.registry.yml logs --tail=100 admin
```

For a fresh installation, first follow the secret/database/certificate setup in
`README.md`, using both Compose files with every command. Replace `build admin`
with `pull admin`; use `up -d --no-build` once the prerequisites are ready.
Always retain the registry override on subsequent operations. The original
`compose.yml` still supports building local source for development.

For a probe already registered with Fleet:

```sh
docker compose -f compose.yml -f compose.registry.yml --profile probe pull probe
docker compose -f compose.yml -f compose.registry.yml --profile probe up -d --no-build --no-deps probe
```

## Node on a separate host

For an existing `/opt/telemt-node/compose.yml`, retain its project name, service
name, mounts, networking and health check. Remove only the node's `build:` block
and set its `image:` to:

```yaml
image: "ghcr.io/efinskiy/telemt-node:${TELEMT_IMAGE_TAG:?Set TELEMT_IMAGE_TAG}"
```

Set `TELEMT_IMAGE_TAG` in `/opt/telemt-node/.env` or export it in the shell. This
is independent of the Fleet host's `.env`. From `/opt/telemt-node`:

```sh
docker compose -f compose.yml pull node
docker compose -f compose.yml up -d --no-build --no-deps node
docker compose -f compose.yml logs --tail=100 node
```

For a fresh standalone node, use the provided `compose.node.yml`. It requires
only `NODE_CONFIG_DIR` (an existing directory containing `config.toml`),
`TELEMT_IMAGE_TAG`, and optionally `TELEMT_IMAGE_PREFIX`. It does not require
Fleet database secrets or a probe reference URL. From this directory:

```sh
docker compose -f compose.node.yml pull node
docker compose -f compose.node.yml up -d --no-build node
```

Do not switch an existing installation to a different project name or volume
layout as part of an image upgrade. Keep the node's HTTPS reverse proxy and
certificates in their existing deployment. Recreating a node interrupts its
open TCP connections; update one node at a time.

## Rollback

Record the deployed image digest/SHA before updating. To revert an application
image, set `TELEMT_IMAGE_TAG` to the previous published SHA and repeat `pull` and
`up --no-build` for that service. Do not use `down -v`. Restore a matching
configuration/database backup if the new version changed fields or schema that
the old version cannot read. Image rollback alone does not undo migrations.

## Build cache and speed

The Dockerfile uses pinned cargo-chef 0.1.71 to build dependencies in a layer
keyed by Cargo manifests/lockfiles and target layout. Source-only edits can reuse
that layer. GitHub exports intermediate layers with `mode=max`, separately for
each image and architecture. Native runners avoid CPU emulation. Local builders
also reuse these ordinary layers. Cache eviction, dependency changes and first
builds still require a full compile.

`CARGO_BUILD_JOBS` continues to default to the builder's CPU count. Rust 1.88,
release optimization, fat LTO and one codegen unit remain unchanged. The final
node compilation/link can still take substantial time. No measured speedup is
claimed until a cold and warm GitHub run have completed. Rust tests never run
inside image builds or on service startup.

References: [cargo-chef](https://github.com/LukeMathWalker/cargo-chef),
[Docker GitHub cache](https://docs.docker.com/build/cache/backends/gha/),
[GHCR authentication](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry).
