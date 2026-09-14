# Monoize v1.9.3 release and deployment

- Release commit: `c0bd5e193a0f45351ac7b24dc70734658f29fd17`.
- Protocol fixes: `3051b09`.
- `master` and annotated tag `v1.9.3` were pushed atomically.
- GitHub release: https://github.com/Ikaleio/monoize/releases/tag/v1.9.3
- Release workflow: https://github.com/Ikaleio/monoize/actions/runs/34679211504
- Validation: `cargo test --locked --all-targets` passed all 266 tests. See `tests.log`.
- Unrelated documentation, E2E, instruction, and artifact changes remain outside these commits.

## Distribution verification

- Release workflow `34679211504` completed successfully. All 11 jobs passed at the release commit.
- The public GitHub release contains all six native archive and checksum files.
- Linux x64 and ARM64 archives use the musl targets. The Windows x64 archive uses the MSVC target.
- The release checks verified archive integrity and Linux static linking.
- Bun, npm, and pnpm installation checks passed.
- The amd64 and ARM64 container builds and multi-architecture publication passed.
- The npm registry reports `monoize@latest` and `monoize@1.9.3` as version `1.9.3`.
- The root npm package binds all three platform package versions to this release.
- No release job failed or required a rerun.

## Ikaleio-TYO deployment

The server built the release commit in `/home/ikaleio/projects/monoize/.git/deploy-worktrees/v1.9.3`.
The deployment used the existing project script, `deploy.sh deploy-watchdog`, with Rust 1.92.
The server completed the release build, retained a backup, replaced `/opt/monoize/monoize`, and restarted PM2.

- PM2 process: `monoize`, PID `200658`, status `online`, restart count `7`.
- Build, deployed file, and running executable SHA-256: `a8fb1da656f019654965bf49c0831b07671f2f105b00eeb7964d9718a632541d`.
- Local and public `/` returned HTTP 200.
- Local and public `/api/dashboard/settings/public` returned HTTP 200 with the configured public API URL.
- Local and public unauthenticated `/v1/models` returned HTTP 401.
- Local and public JavaScript asset `/assets/index-BMzRcRtu.js` matched the build artifact.
- Asset SHA-256: `0fa523053d981c0825eeb4dc6d7d1bf9ef9603d601f22122229035b122b5b19c`.
- The rollback watchdog was cancelled after these checks passed.
- PM2 retained the same PID and restart count after watchdog cancellation.

See `tyo-deploy.log`, `tyo-verification.log`, and `verify-tyo.py` for evidence and the verification procedure.
These deployment checks confirm service availability and artifact identity. They do not establish upstream Provider acceptance of all protocol features.
