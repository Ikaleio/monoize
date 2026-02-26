#!/usr/bin/env bash
set -euo pipefail

DEPLOY_DIR="/opt/monoize"
BINARY_NAME="monoize"
PM2_NAME="monoize"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

step() { echo -e "${GREEN}[DEPLOY]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; exit 1; }

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

# --- 4. Atomic swap ---
step "Deploying binary to $DEPLOY_DIR..."
cp "$SCRIPT_DIR/target/release/$BINARY_NAME" "$DEPLOY_DIR/${BINARY_NAME}.next"
mv "$DEPLOY_DIR/${BINARY_NAME}.next" "$DEPLOY_DIR/$BINARY_NAME"

# --- 5. Restart ---
step "Restarting PM2 process..."
pm2 restart "$PM2_NAME" || fail "PM2 restart failed"
pm2 save || warn "PM2 save failed (non-fatal)"

step "Deploy complete."
