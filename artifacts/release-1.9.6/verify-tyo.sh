set -eu
cd /home/ikaleio/projects/monoize/.git/deploy-worktrees/v1.9.6
MONO_PID=$(pm2 jlist | jq -r '.[] | select(.name == "monoize" and .pm2_env.status == "online") | .pid')
test -n "$MONO_PID"
printf 'pid=%s\n' "$MONO_PID"
sha256sum target/release/monoize /opt/monoize/monoize "/proc/$MONO_PID/exe"
MONO_BUILD_HASH=$(sha256sum target/release/monoize | cut -d ' ' -f1)
MONO_ACTIVE_HASH=$(sha256sum "/proc/$MONO_PID/exe" | cut -d ' ' -f1)
test "$MONO_BUILD_HASH" = "$MONO_ACTIVE_HASH"
printf 'local_home='; curl --fail -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:40550/
printf 'public_home='; curl --fail -s -o /dev/null -w '%{http_code}\n' https://mono.ikale.io/
printf 'unauth_models='; curl -s -o /dev/null -w '%{http_code}\n' https://mono.ikale.io/v1/models
./deploy.sh cancel-watchdog
