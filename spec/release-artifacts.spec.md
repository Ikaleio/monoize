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
| Linux x86-64 | `ubuntu-24.04` | `x86_64-unknown-linux-musl` |
| Linux ARM64 | `ubuntu-24.04-arm` | `aarch64-unknown-linux-musl` |
| macOS x86-64 | `macos-15-intel` | `x86_64-apple-darwin` |
| macOS ARM64 | `macos-15` | `aarch64-apple-darwin` |
| Windows x86-64 | `windows-2025` | `x86_64-pc-windows-msvc` |
| Windows ARM64 | `windows-11-arm` | `aarch64-pc-windows-msvc` |

RA-M2. Each row MUST run on a runner whose CPU architecture and operating system match the Rust target. The workflow MUST NOT use CPU emulation or cross-architecture compilation for these six rows. A Linux row MAY compile through a musl toolchain container on its matching native Linux runner.

RA-M2a. Each Linux row MUST use the matching `rust-musl-cross` builder image. The x86-64 image reference MUST equal `ghcr.io/rust-cross/rust-musl-cross:x86_64-musl@sha256:ce75e9174325d4fbb3de85c309e2d7ca29f7500169bc4b5d2c611ff7e86d549a`. The ARM64 image reference MUST equal `ghcr.io/rust-cross/rust-musl-cross:aarch64-musl@sha256:ecae5dd62d1c938c14f8071d36c16fa699860aace03bfb5284fb1216474d2643`.

RA-M2b. After each Linux build, the workflow MUST inspect the executable's ELF program headers and dynamic section. An ELF interpreter or a `DT_NEEDED` entry MUST fail the build row before packaging.

RA-M2c. Each Linux build MUST select the matching musl GCC linker: `x86_64-unknown-linux-musl-gcc` for `x86_64-unknown-linux-musl`, or `aarch64-unknown-linux-musl-gcc` for `aarch64-unknown-linux-musl`. Cargo MUST use `scripts/musl-linker.sh` as the target linker and provide the matching GCC command through `MONOIZE_MUSL_LINKER`. The wrapper MUST apply these ordered rewrites and execute the selected GCC command:

1. Replace every exact `-Wl,-Bdynamic` argument with `-Wl,-Bstatic`.
2. On x86-64, replace every exact `-lstdc++` argument with `-Wl,--start-group -lstdc++ -lc -Wl,--end-group`.
3. On ARM64, replace every exact `-lstdc++` argument with `-Wl,--start-group -lstdc++ -lc -lgcc -Wl,--end-group`.
4. Reject `MONOIZE_MUSL_STATIC_LIBGCC` unless it equals `true` or `false` when present.
5. Preserve the order and bytes of every other argument.

The first rewrite MUST prevent a dependency build script from overriding static linkage by declaring a dynamic native library. The second and third rewrites MUST let the static C++ runtime resolve its musl libc references even when rustc placed the original libc input earlier in the link command. The ARM64 group MUST provide the GCC outline-atomics helpers referenced by the builder image's static C++ runtime.

RA-M2d. Each Linux build MUST pass `-C target-feature=+crt-static -C link-arg=-static -C link-arg=-static-libstdc++` through target Rust flags. Together with RA-M2c, these settings MUST prevent the executable from depending on a host GNU libc, musl libc, or `libstdc++.so.6` shared library.

RA-M3. Matrix `fail-fast` MUST equal `false`. Every matrix row MUST set job-level `continue-on-error = true`. A failed row MUST NOT cancel another row or make the aggregate matrix dependency fail.

RA-M4. A Linux builder image MUST provide the stable Rust toolchain and its row's musl target. Every other row MUST install the stable Rust toolchain with its row's target. Every row MUST install Bun `1.4.0`.

RA-M5. A row MUST run `bun install --frozen-lockfile` in `frontend/` before the Rust build.

RA-M6. A row MUST run `cargo build --locked --release --target <target>`.

RA-M7. A build failure, lockfile mutation requirement, frontend dependency mismatch, or packaging failure MUST fail that matrix row.

RA-M8. After installing Bun and, for a non-Linux row, the toolchain, and before `cargo build`, each native build row MUST restore GitHub Actions caches for:

1. the Bun package download cache, keyed by runner OS, runner architecture, and `frontend/bun.lock`;
2. the Cargo registry, git, and target directories, keyed by runner OS, runner architecture, the row's Rust target, and `Cargo.lock`.

A cache miss MUST continue the job. A cache hit MUST NOT skip `bun install --frozen-lockfile` or `cargo build --locked --release --target <target>`. Restored caches MUST NOT rewrite `Cargo.lock` or `frontend/bun.lock`.

RA-M8a. A Linux builder container MUST mount the restored Cargo registry and git cache directories from the runner. It MUST write build artifacts to the same `target/` directory restored by the Rust cache action.

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

RA-S2. One verification job MUST wait for every build row. The asset-publishing job MUST depend on that verification job. The verification and asset-publishing jobs MUST continue when one or more build rows fail, provided at least one row produced a valid artifact.

RA-S3. The verification job and asset-publishing job MUST each download and merge every available `release-<target>` Actions artifact into one directory.

RA-S4. `scripts/package_release.py verify` MUST require a non-empty subset of the six targets in RA-M1. For each present target, the merged directory MUST contain exactly its archive and checksum file. The command MUST reject an unknown file, an archive without its checksum, or a checksum without its archive.

RA-S5. The verify command MUST recompute and compare every present archive checksum. An unknown, orphaned, malformed, or mismatched file MUST fail verification.

RA-S6. After RA-S4 and RA-S5 succeed, the workflow MUST upload every verified file to the triggering GitHub Release. A rerun MAY overwrite same-name assets on that Release.

RA-S6a. A failed target MUST contribute no native archive, checksum, or npm platform package. Its failure MUST NOT block verification, GitHub Release upload, npm publication, or container publication for successful targets.

RA-S7. The release workflow MUST NOT run `deploy.sh`, copy files to `/opt/monoize`, restart PM2, or mutate a Monoize database.

RA-S8. Each native build row MUST make its compiled executable available to the matching npm platform-package staging step. npm packaging and publication MUST follow `spec/npm-cli-distribution.spec.md`.

RA-S9. Failure to publish an npm package MUST NOT delete or replace a verified native GitHub Release asset.
