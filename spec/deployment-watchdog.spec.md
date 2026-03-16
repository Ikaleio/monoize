# Deployment Watchdog Specification

## 0. Scope

- This specification defines the behavior of the repository-local `deploy.sh` script.
- The deployment target directory is `/opt/monoize`.
- The deployed process name is `monoize` under PM2.

## 1. Default deploy workflow

D1. Running `./deploy.sh` with no subcommand MUST execute the following steps in order:

1. Build the frontend with `bun run build` from `frontend/`.
2. Build the release binary with `cargo build --release` from the repository root.
3. If `/opt/monoize/monoize` exists, copy it to `/opt/monoize/monoize.bak.<timestamp>` before replacing it.
4. Copy `target/release/monoize` to `/opt/monoize/monoize.next` and atomically move it to `/opt/monoize/monoize`.
5. Restart PM2 process `monoize`.
6. Save PM2 state.

D2. Before step D1.4 completes, the script MUST cancel any previously armed deployment watchdog state recorded under `/opt/monoize/.deploy-watchdog/`.

D2a. If step D1.5 fails and a backup path from D1.3 exists, the script MUST synchronously restore that backup binary to `/opt/monoize/monoize`, attempt to restart PM2 process `monoize` using the restored binary, and then exit with failure.

## 2. Watchdog arming behavior

D3. After a deploy completes step D1.6 and a backup path from D1.3 exists, the script MUST arm a rollback watchdog with a timeout of exactly 300 seconds.

D4. The watchdog MUST persist its state under `/opt/monoize/.deploy-watchdog/` using files that identify:

- the currently armed deploy identifier;
- the PID of the background watchdog process;
- the backup binary path to restore.

D5. While the watchdog is armed, the repository operator MUST be able to disarm it by running `./deploy.sh cancel-watchdog`.

D6. `./deploy.sh cancel-watchdog` MUST:

- terminate the currently armed watchdog process if it is still running;
- remove the armed deploy identifier and metadata files;
- leave the current deployed binary unchanged.

## 3. Automatic rollback behavior

D7. When the 300-second watchdog timeout expires, the watchdog MUST check whether the same deploy identifier is still armed.

D8. If the deploy identifier is still armed and the recorded backup binary still exists, the watchdog MUST:

1. copy the recorded backup binary to `/opt/monoize/monoize.rollback`;
2. atomically move `/opt/monoize/monoize.rollback` to `/opt/monoize/monoize`;
3. restart PM2 process `monoize`;
4. save PM2 state;
5. clear the watchdog armed state files.

D9. If the timeout expires but either the deploy identifier is no longer armed or the recorded backup binary does not exist, the watchdog MUST exit without modifying the deployed binary.
