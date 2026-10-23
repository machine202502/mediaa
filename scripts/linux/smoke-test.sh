#!/usr/bin/env bash
set -u
export DEBIAN_FRONTEND=noninteractive
bash /src/scripts/linux/install-runtime-deps.sh >/tmp/deps.log 2>&1 || { tail -20 /tmp/deps.log; exit 1; }
apt-get install -y --no-install-recommends xvfb imagemagick dbus-x11 >/dev/null 2>&1

cp /src/dist/linux/mediaa /usr/local/bin/mediaa && chmod +x /usr/local/bin/mediaa
missing="$(ldd /usr/local/bin/mediaa | grep 'not found' || true)"
if [[ -n "$missing" ]]; then
  echo "missing libraries:"; echo "$missing"; exit 1
fi
echo "ldd: all libraries resolved"

Xvfb :99 -screen 0 1600x1000x24 >/dev/null 2>&1 &
sleep 1
export DISPLAY=:99 LANG="${LANG:-en_US.UTF-8}"
dbus-launch --exit-with-session mediaa >/tmp/run.log 2>&1 &
sleep 8
import -window root /dist/smoke.png
if pgrep -x mediaa >/dev/null; then
  echo "OK: MediaA is running; screenshot: dist/linux/smoke.png"
else
  echo "FAILED: MediaA exited"; cat /tmp/run.log; exit 1
fi
