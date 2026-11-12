# Continuous integration

## Scope

CI-1. The workflow `.github/workflows/check.yml` MUST run on each push to `master` and each pull request.
It MUST use an `ubuntu-24.04` runner.

## Toolchain and dependencies

CI-2. The workflow MUST install stable Rust with the `rustfmt` and `clippy` components.
It MUST install Bun version `1.4.0`.
It MUST run `bun install --frozen-lockfile` from `frontend/` before Rust compilation because `build.rs` builds the dashboard.

CI-3. The workflow MUST cache the Bun install cache and Rust build artifacts with the actions used in `build-native.yml`.
The Bun cache key MUST include the runner operating system, runner architecture, and the hash of `frontend/bun.lock`.

## Required gates

CI-4. From the repository root, the workflow MUST run these commands in this order:

1. `cargo fmt --all --check`
2. `cargo clippy --locked --lib --tests -- -D warnings`
3. `cargo test --locked --lib`

CI-5. From `frontend/`, the workflow MUST run these commands after the Rust gates:

1. `bunx tsc -p tsconfig.app.json --noEmit`
2. `bun run lint`

CI-6. Each gate MUST fail when its command returns a nonzero exit code.
The workflow MUST report success only when dependency installation and all five gates return zero.
The workflow MUST NOT suppress compiler warnings or Clippy warnings.
