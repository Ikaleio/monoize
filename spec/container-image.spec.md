# Container Image Specification

## 0. Status

- **Purpose:** Build, run, and publish the Monoize Linux container image.
- **Scope:** Applies to `Dockerfile`, `.dockerignore`, `.github/workflows/release.yml`, `README.md`, and `README.zh-CN.md`.

## 1. Runtime image

CI-R1. The final image MUST contain the release-mode `monoize` executable with the embedded dashboard.

CI-R2. The final image MUST NOT contain Rust, Bun, source files, build caches, or frontend dependencies.

CI-R3. The final image MUST run `monoize` as a non-root user named `monoize`.

CI-R4. The image working directory MUST be `/app`.

CI-R5. The image MUST create `/app/data` and declare it as a volume. With the default database configuration, Monoize MUST store its SQLite database at `/app/data/monoize.db`.

CI-R6. The image MUST expose TCP port `8080`. Monoize MUST retain `0.0.0.0:8080` as its default listen address.

CI-R7. The image MUST define an HTTP health check for `http://127.0.0.1:8080/`. The health check MUST use a 30-second interval, a 5-second timeout, a 10-second start period, and three retries.

CI-R8. The final image MUST include CA certificates so that Monoize can establish TLS connections to upstream services.

CI-R9. The image MUST declare these Open Container Initiative labels:

- `org.opencontainers.image.title=Monoize`;
- `org.opencontainers.image.description` with a factual product description;
- `org.opencontainers.image.source=https://github.com/Ikaleio/monoize`;
- `org.opencontainers.image.licenses=MIT`;
- `org.opencontainers.image.version` from the `VERSION` build argument;
- `org.opencontainers.image.revision` from the `REVISION` build argument.

## 2. Build inputs

CI-B1. The runtime base image MUST be Ubuntu 24.04, referenced by a multi-platform manifest digest. The Monoize executable copied into the image MUST be the static musl executable produced by the matching Linux row in `release-artifacts.spec.md` RA-M1.

CI-B2. The image build MUST NOT install Rust, Bun, clang, cmake, or a C/C++ toolchain.

CI-B3. Every base image reference MUST include a multi-platform manifest digest.

CI-B4. Each platform container job MUST download the native Release archive for the matching RA-M1 target, verify its sibling SHA-256 file, and extract the executable:

| Container platform | Native Rust target |
| --- | --- |
| `linux/amd64` | `x86_64-unknown-linux-musl` |
| `linux/arm64` | `aarch64-unknown-linux-musl` |

CI-B5. The image build MUST copy that extracted executable to `/usr/local/bin/monoize` with mode `0755`. It MUST NOT run `cargo`, `bun`, `rustc`, or compile Monoize from source.

CI-B6. The Docker build context MUST contain exactly the `Dockerfile` and one file named `monoize` (the extracted native executable). It MUST NOT contain Git metadata, source trees, frontend dependencies, Cargo `target/` output, or other Release archives.

CI-B7. Each platform container job MUST restore and save GitHub Actions Docker layer cache scoped to that platform (`linux-amd64` or `linux-arm64`). A cache miss MUST continue the job. A cache hit MUST NOT skip copying the native executable or change `VERSION` / `REVISION` build arguments.

## 3. Publication authority and triggers

CI-P1. The publication workflow MUST publish to `ghcr.io/<lowercase github.repository>`.

CI-P2. The workflow MUST use `GITHUB_TOKEN` with `contents: read` and `packages: write`. It MUST NOT require a personal access token.

CI-P3. The container jobs MUST be part of `.github/workflows/release.yml`. They MUST run on a published GitHub Release. A manual workflow run MUST run the container jobs only when `publish_container` is true.

CI-P4. For a GitHub Release, the workflow MUST check out `github.event.release.tag_name`. The tag MUST pass the release-tag and Cargo-version validation in `scripts/package_release.py` before a container build starts.

CI-P5. A manual run MUST accept a Git ref, a `publish_container` boolean, and one container tag. When `publish_container` is true, it MUST check out that ref and publish only that exact container tag.

CI-P5a. The container build MUST use the immutable source commit SHA resolved by the validation job. It MUST NOT resolve the release tag or manual Git ref again.

CI-P6. A manual container tag MUST match `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,127}$`.

CI-P7. Concurrent workflow runs that target the same publication tag MUST execute sequentially. A newer run MUST NOT cancel an active run.

CI-P8. For a GitHub Release, a container platform build MUST start only after the workflow uploads the available verified native Release assets successfully. A missing native archive MUST skip only the matching container platform.

CI-P9. For a manual run with `publish_container = true`, the container build MUST start only after the available native artifacts pass checksum verification.

## 4. Platforms and tags

CI-M1. One publication MUST attempt exactly these platforms on native GitHub-hosted runners:

| Platform | Runner |
| --- | --- |
| `linux/amd64` | `ubuntu-24.04` |
| `linux/arm64` | `ubuntu-24.04-arm` |

CI-M1a. The two platform container jobs MAY run in parallel. They MUST NOT compile Monoize; each job only packages the native linux executable for its architecture.

CI-M2. Each successful platform build MUST push a content-addressed image. The merge job MUST create one manifest list from the one or two available digests.

CI-M3. The merge job MUST wait for both platform attempts. A failed or skipped platform MUST NOT block publication when the other platform produced a digest. If neither platform produced a digest, the container publication steps MUST skip without blocking GitHub Release or npm publication.

CI-M4. A Release tag `vMAJOR.MINOR.PATCH` MUST publish these tags:

- `vMAJOR.MINOR.PATCH`;
- `MAJOR.MINOR.PATCH`;
- `MAJOR.MINOR`;
- `MAJOR`;
- `latest`.

CI-M5. The workflow MUST inspect the published manifest after it creates all tags. A manifest creation or inspection failure MUST fail the workflow.

## 5. User documentation

CI-D1. Both READMEs MUST document the same `docker run` command.

CI-D2. The documented command MUST publish host port `8080`, mount a named volume at `/app/data`, and use `ghcr.io/ikaleio/monoize:latest`.

CI-D3. Both READMEs MUST identify `MONOIZE_DATABASE_DSN` as the method to select PostgreSQL or a non-default SQLite location.
