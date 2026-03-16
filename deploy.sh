#!/usr/bin/env bash
set -euo pipefail

DEPLOY_DIR="/opt/monoize"
BINARY_NAME="monoize"
PM2_NAME="monoize"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WATCHDOG_DIR="$DEPLOY_DIR/.deploy-watchdog"
WATCHDOG_ID_FILE="$WATCHDOG_DIR/current_id"
WATCHDOG_PID_FILE="$WATCHDOG_DIR/current_pid"
WATCHDOG_META_FILE="$WATCHDOG_DIR/current_backup"
WATCHDOG_TIMEOUT_SECONDS=300

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

step() { echo -e "${GREEN}[DEPLOY]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; exit 1; }

ensure_watchdog_dir() {
    mkdir -p "$WATCHDOG_DIR"
}

clear_watchdog_state() {
    rm -f "$WATCHDOG_ID_FILE" "$WATCHDOG_PID_FILE" "$WATCHDOG_META_FILE"
}

cancel_watchdog() {
    ensure_watchdog_dir
    if [ -f "$WATCHDOG_PID_FILE" ]; then
        local pid
        pid="$(cat "$WATCHDOG_PID_FILE")"
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            kill "$pid" 2>/dev/null || true
        fi
    fi
    clear_watchdog_state
}

rollback_binary() {
    local backup_path="$1"
    cp "$backup_path" "$DEPLOY_DIR/${BINARY_NAME}.rollback"
    mv "$DEPLOY_DIR/${BINARY_NAME}.rollback" "$DEPLOY_DIR/$BINARY_NAME"
}

restore_backup_after_restart_failure() {
    local backup_path="$1"
    warn "PM2 restart failed; restoring backup binary from $backup_path"
    rollback_binary "$backup_path"
    if ! pm2 restart "$PM2_NAME"; then
        fail "PM2 restart failed after restoring backup binary"
    fi
    pm2 save || warn "PM2 save failed after restoring backup (non-fatal)"
    fail "PM2 restart failed for new binary; restored previous binary"
}

arm_watchdog() {
    local backup_path="$1"
    ensure_watchdog_dir
    local deploy_id
    deploy_id="$(date +%Y%m%d%H%M%S)-$$"
    printf '%s\n' "$deploy_id" > "$WATCHDOG_ID_FILE"
    printf '%s\n' "$backup_path" > "$WATCHDOG_META_FILE"

    (
        sleep "$WATCHDOG_TIMEOUT_SECONDS"

        if [ ! -f "$WATCHDOG_ID_FILE" ] || [ "$(cat "$WATCHDOG_ID_FILE")" != "$deploy_id" ]; then
            exit 0
        fi

        if [ ! -f "$backup_path" ]; then
            rm -f "$WATCHDOG_ID_FILE" "$WATCHDOG_PID_FILE" "$WATCHDOG_META_FILE"
            exit 0
        fi

        rollback_binary "$backup_path"
        pm2 restart "$PM2_NAME"
        pm2 save || true
        rm -f "$WATCHDOG_ID_FILE" "$WATCHDOG_PID_FILE" "$WATCHDOG_META_FILE"
    ) >/dev/null 2>&1 &

    printf '%s\n' "$!" > "$WATCHDOG_PID_FILE"
    step "Rollback watchdog armed for ${WATCHDOG_TIMEOUT_SECONDS}s. Run ./deploy.sh cancel-watchdog to keep the new binary."
}

case "${1:-deploy}" in
    cancel-watchdog)
        cancel_watchdog
        step "Rollback watchdog cancelled."
        exit 0
        ;;
    deploy)
        ;;
    *)
        fail "Unknown subcommand: ${1}. Supported: deploy, cancel-watchdog"
        ;;
esac

ensure_watchdog_dir
cancel_watchdog

BACKUP=""

# --- 1. Frontend build ---
step "Building frontend..."
(cd "$SCRIPT_DIR/frontend" && bun run build) || fail "Frontend build failed"

# --- 2. Release build ---
step "Building release binary..."
(cd "$SCRIPT_DIR" && cargo build --release) || fail "Cargo release build failed"

# --- 3. Backup current binary ---
if [ -f "$DEPLOY_DIR/$BINARY_NAME" ]; then
    BACKUP="$DEPLOY_DIR/${BINARY_NAME}.bak.$(date +%Y%m%d%H%M%S)"
    step "Backing up current binary to $BACKUP"
    cp "$DEPLOY_DIR/$BINARY_NAME" "$BACKUP"
    # Keep only the 3 most recent backups
    ls -t "$DEPLOY_DIR"/${BINARY_NAME}.bak.* 2>/dev/null | tail -n +4 | xargs -r rm -f
fi

# --- 4. Atomic swap + restart ---
step "Deploying binary to $DEPLOY_DIR..."
cp "$SCRIPT_DIR/target/release/$BINARY_NAME" "$DEPLOY_DIR/${BINARY_NAME}.next"
mv "$DEPLOY_DIR/${BINARY_NAME}.next" "$DEPLOY_DIR/$BINARY_NAME"

step "Restarting PM2 process..."
if ! pm2 restart "$PM2_NAME"; then
    if [ -n "$BACKUP" ]; then
        restore_backup_after_restart_failure "$BACKUP"
    fi
    fail "PM2 restart failed"
fi
pm2 save || warn "PM2 save failed (non-fatal)"

if [ -n "$BACKUP" ]; then
    arm_watchdog "$BACKUP"
else
    warn "No previous binary found; rollback watchdog not armed."
fi

step "Deploy complete."
