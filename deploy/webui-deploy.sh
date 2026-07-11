#!/usr/bin/env bash
# Build the SvelteKit UI and deploy it to the xserver paur host.
#
# The Caddyfile (see deploy/Caddyfile) serves the UI from
# /var/lib/paur/webui/_app on the build host. This script:
#   1. runs `npm run build` in web/
#   2. rsyncs the build output to xserver:/tmp/paur-web-build
#      (rsync over ssh, no sudo on the local side needed because
#      /tmp is writable by the user)
#   3. ssh + sudo to atomically swap /tmp/paur-web-build into
#      /var/lib/paur/webui (which is the path Caddy reads)
#
# The previous flow (rsync --delete to a `web/` dir on xserver)
# silently mis-deployed: Caddy reads `webui/`, not `web/`, so the
# new bundle never reached clients. We bake that path into the
# script so it can't drift again.
#
# Required environment / config:
#   SSH_USER  — ssh user on xserver (default: user)
#   SSH_HOST  — target host (default: xserver)
#   SSH_KEY   — path to identity file (default: ~/.ssh/paur_xserver)
#   PAUR_DATA_DIR — data dir on xserver (default: /var/lib/paur)
#
# The first run requires the local ssh key to be authorized on
# xserver. The script does NOT configure that — run
# `ssh-copy-id -i $SSH_KEY $SSH_USER@$SSH_HOST` once.

set -euo pipefail

SSH_USER="${SSH_USER:-user}"
SSH_HOST="${SSH_HOST:-xserver}"
SSH_KEY="${SSH_KEY:-$HOME/.ssh/paur_xserver}"
PAUR_DATA_DIR="${PAUR_DATA_DIR:-/var/lib/paur}"
WEBUI_DIR="${PAUR_DATA_DIR}/webui"

# Resolve the web/ directory relative to this script so the script
# works regardless of where it's invoked from.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WEB_DIR="${SCRIPT_DIR}/../web"

if [[ ! -d "${WEB_DIR}" ]]; then
    echo "error: web/ directory not found at ${WEB_DIR}" >&2
    exit 1
fi

SSH_TARGET="${SSH_USER}@${SSH_HOST}"
SSH_OPTS=(-i "${SSH_KEY}" -o StrictHostKeyChecking=no)
RSYNC_SSH="ssh ${SSH_OPTS[*]}"

echo "==> Building SvelteKit UI in ${WEB_DIR}/"
( cd "${WEB_DIR}" && npm run build )

if [[ ! -f "${WEB_DIR}/build/index.html" ]]; then
    echo "error: build/index.html missing — npm run build failed?" >&2
    exit 1
fi

echo "==> Rsyncing build/ to ${SSH_TARGET}:/tmp/paur-web-build/"
rsync -avz --delete \
    -e "${RSYNC_SSH}" \
    "${WEB_DIR}/build/" \
    "${SSH_TARGET}:/tmp/paur-web-build/"

echo "==> Swapping /tmp/paur-web-build into ${WEBUI_DIR}/ on xserver"
ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" \
    "sudo rm -rf '${WEBUI_DIR}' && sudo mv /tmp/paur-web-build '${WEBUI_DIR}'"

echo "==> Verifying Caddy picks up the new bundle"
# Hit the install page node bundle by reading the page first,
# then fetch the referenced chunk. We don't hard-code the hash
# because the bundle filename changes on every build.
NEW_BUNDLE=$(ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" \
    "sudo ls '${WEBUI_DIR}/_app/immutable/nodes/' | sort -n" | tail -1)
echo "    newest node bundle: ${NEW_BUNDLE}"
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" \
    "http://${SSH_HOST}/_app/immutable/nodes/${NEW_BUNDLE}")
if [[ "${HTTP_CODE}" != "200" ]]; then
    echo "error: Caddy returned ${HTTP_CODE} for ${NEW_BUNDLE}" >&2
    exit 1
fi
echo "    Caddy returned 200"

echo "==> Done. Visit http://${SSH_HOST}/install to verify."
