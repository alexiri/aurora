#!/bin/bash
# Chromium wrapper (docs/12 §Loopback proxies, RFE-003). Browsers use the open proxy
# 127.0.0.1:3129, which the supervisor starts only when the sandbox booted with the
# `browser` capability; its loopback forwarder adds the credentials, so none are given here.
set -u
if ! (exec 3<>/dev/tcp/127.0.0.1/3129) 2>/dev/null; then
    echo "browsing is disabled for this workspace" >&2
    exit 1
fi
for real in /usr/bin/chromium-browser /usr/bin/chromium; do
    [ -x "$real" ] && exec "$real" --proxy-server=http://127.0.0.1:3129 --proxy-bypass-list='<-loopback>' "$@"
done
echo "chromium is not installed in this image" >&2
exit 127
