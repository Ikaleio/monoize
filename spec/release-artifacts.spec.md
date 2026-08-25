# Release Artifacts Specification

## 0. Status

- **Purpose:** Build and attach native Monoize binaries when a GitHub Release is published.
- **Scope:** Applies to `.github/workflows/release.yml`, `scripts/package_release.py`, and the native inputs used by the npm package set.

## 1. Trigger and authority

RA-T1. The release workflow MUST run on the GitHub `release` event with `types = [published]`.

RA-T2. Creating or editing a draft Release MUST NOT run the workflow.

RA-T3. The workflow MUST check out `github.event.release.tag_name` rather than the default branch head.

RA-T3a. The workflow MAY expose a manual preflight trigger with explicit `ref` and `tag` inputs. A manual preflight MUST execute validation, all six builds, packaging, checksum verification, and Actions-artifact staging. It MUST NOT upload files to a GitHub Release.

RA-T3b. The validation job MUST resolve the checked-out release ref to one commit SHA. Every build, verification, and container job in the same run MUST check out that exact commit SHA. A later movement of a branch or tag ref MUST NOT change the source revision used by any job in the run.

RA-T4. The release tag MUST equal the literal character `v` followed by the `[package].version` value in `Cargo.toml`. A mismatch MUST fail before compilation and MUST upload no Release assets.

RA-T5. Build jobs MUST have `contents: read` permission. Only the asset-publishing job MAY have `contents: write` permission.

RA-T5a. Native build jobs MAY have `actions: write` solely to restore and save the GitHub Actions caches defined by RA-M8. They MUST NOT have `contents: write`.

RA-T6. Every third-party or GitHub-provided action reference MUST use a full commit SHA. A comment on the same line MUST identify the corresponding release tag or major version.

## 2. Native build matrix

RA-M1. One workflow run MUST contain exactly these native build rows:

| Operating system | Runner label | Rust target |
| --- | --- | --- |
| Linux x86-64 | `ubuntu-24.04` | `x86_64-unknown-linux-gnu` |
| Linux ARM64 | `ubuntu-24.04-arm` | `aarch64-unknown-linux-gnu` |
| macOS x86-64 | `macos-15-intel` | `x86_64-apple-darwin` |
| macOS ARM64 | `macos-15` | `aarch64-apple-darwin` |
| Windows x86-64 | `windows-2025` | `x86_64-pc-windows-msvc` |
| Windows ARM64 | `windows-11-arm` | `aarch64-pc-windows-msvc` |

RA-M2. Each row MUST compile on a runner whose native architecture and operating system match the Rust target. The workflow MUST NOT use emulation or cross-compilation for these six rows.

RA-M3. Matrix `fail-fast` MUST equal `false`. A failed row MUST NOT cancel another running row.

RA-M4. A row MUST install the stable Rust toolchain with the row's target and Bun `1.4.0`.

RA-M5. A row MUST run `bun install --frozen-lockfile` in `frontend/` before the Rust build.

RA-M6. A row MUST run `cargo build --locked --release --target <target>`.

RA-M7. A build failure, lockfile mutation requirement, frontend dependency mismatch, or packaging failure MUST fail that matrix row.

RA-M8. After installing the toolchain and Bun, and before `cargo build`, each native build row MUST restore GitHub Actions caches for:

1. the Bun package download cache, keyed by runner OS, runner architecture, and `frontend/bun.lock`;
2. the Cargo registry, git, and target directories, keyed by runner OS, runner architecture, the row's Rust target, and `Cargo.lock`.

A cache miss MUST continue the job. A cache hit MUST NOT skip `bun install --frozen-lockfile` or `cargo build --locked --release --target <target>`. Restored caches MUST NOT rewrite `Cargo.lock` or `frontend/bun.lock`.

RA-M9. After a native build row finishes compiling, the workflow MUST save the caches in RA-M8. A cache save failure MUST NOT fail the job.

## 3. Package contents and names

RA-P1. `scripts/package_release.py package` MUST accept a release tag, one Rust target from RA-M1, and an output directory.

RA-P2. The package command MUST derive the product version from `Cargo.toml` and enforce RA-T4.

RA-P3. The package command MUST read the executable from `target/<target>/release/monoize` on Linux and macOS, or `target/<target>/release/monoize.exe` on Windows.

RA-P4. One archive MUST contain exactly one top-level directory named `monoize-<tag>-<target>`. That directory MUST contain:

- `monoize` on Linux and macOS, or `monoize.exe` on Windows;
- `LICENSE`;
- `README.md`;
- `README.zh-CN.md`.

RA-P5. Linux and macOS archives MUST use the name `monoize-<tag>-<target>.tar.gz`. Windows archives MUST use the name `monoize-<tag>-<target>.zip`.

RA-P6. A tar archive MUST store the executable with mode `0755`. It MUST store documentation files with mode `0644`.

RA-P7. Each archive MUST have a sibling `<archive-name>.sha256` file. Its UTF-8 content MUST equal the lowercase SHA-256 digest, two ASCII spaces, the archive basename, and one newline.

RA-P8. Archive entries MUST use fixed timestamps and owner metadata. Repeating the package command over byte-identical inputs with the same Python and compression implementation MUST produce byte-identical output.

## 4. Staging and publication

RA-S1. Each matrix row MUST upload its archive and checksum as one Actions artifact named `release-<target>`.

RA-S2. One verification job MUST depend on the complete build matrix. The asset-publishing job MUST depend on that verification job. The asset-publishing job MUST run only when every build row and verification succeed and the workflow event is `release`.

RA-S3. The verification job and asset-publishing job MUST each download and merge all six Actions artifacts into one directory.

RA-S4. `scripts/package_release.py verify` MUST reject the merged directory unless it contains exactly the six archives and six checksum files required by RA-M1 and RA-P5 through RA-P7.

RA-S5. The verify command MUST recompute and compare every archive checksum. A missing, additional, malformed, or mismatched file MUST fail verification.

RA-S6. After RA-S4 and RA-S5 succeed, the workflow MUST upload all twelve files to the triggering GitHub Release. A rerun MAY overwrite same-name assets on that Release.

RA-S7. The release workflow MUST NOT run `deploy.sh`, copy files to `/opt/monoize`, restart PM2, or mutate a Monoize database.

RA-S8. Each native build row MUST make its compiled executable available to the matching npm platform-package staging step. npm packaging and publication MUST follow `spec/npm-cli-distribution.spec.md`.

RA-S9. Failure to publish an npm package MUST NOT delete or replace a verified native GitHub Release asset.
