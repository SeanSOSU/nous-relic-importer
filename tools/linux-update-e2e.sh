#!/bin/bash
# The Linux updater end to end, inside tools/linux.Dockerfile's image, as root:
#   tools/linux-update-e2e.sh <older build> <feed folder>
# <feed folder> holds latest.json and the newer build (nri-release sign --base-url http://127.0.0.1:8765/), signed
# with the key the older build trusts. A normal user installs the older build and switches starting at login off.
# It finds the newer version, downloads and verifies it, and waits: on Linux the new file needs the password again,
# so it installs on Install. Install from the window hands over: the old copy quits, the new build replaces it, is
# granted the capability through the stand-in pkexec and runs, and starting at login stays off.
set -euo pipefail
OLD=$1
FEED=$2
HERE=$(cd "$(dirname "$0")" && pwd)
E2E="python3 $HERE/linux_e2e.py"
fail() { echo "FAIL: $*"; exit 1; }
wait_for() { for _ in $(seq 1 "${2:-100}"); do if eval "$1"; then return 0; fi; sleep 0.1; done; fail "timed out: $1"; }

id tester >/dev/null 2>&1 || useradd -m tester
echo 'tester ALL=(root) NOPASSWD: ALL' > /etc/sudoers.d/tester
printf '#!/bin/sh\nexec sudo -n "$@"\n' > /usr/local/bin/pkexec
printf '#!/bin/sh\necho "$1" >> /tmp/opened.txt\n' > /usr/local/bin/xdg-open
chmod 755 /usr/local/bin/pkexec /usr/local/bin/xdg-open
rm -f /tmp/opened.txt
python3 -m http.server 8765 --bind 127.0.0.1 --directory "$FEED" >/tmp/feed.log 2>&1 &
install -m 755 "$OLD" /tmp/nous-relic-importer
NEW="$FEED/nous-relic-importer"
FEED_URL=http://127.0.0.1:8765/latest.json
H=/home/tester
INSTALLED=$H/.local/share/nous-relic-importer/nous-relic-importer
AUTOSTART=$H/.config/autostart/nous-relic-importer.desktop

echo "== install the older build, pointed at the test feed"
su tester -c "/tmp/nous-relic-importer --update-feed $FEED_URL"
wait_for "ss -ltn | grep -q 127.0.0.1:23314"
grep -q -- "--update-feed" "$AUTOSTART" || fail "the feed did not carry into the login autostart"
OLD_PID=$(pgrep -f "^$INSTALLED" | head -1)
su tester -c "$INSTALLED"
wait_for "grep -q '^http://127.0.0.1:23314/?token=' /tmp/opened.txt"
TOKEN=$(grep '^http://127.0.0.1:23314/?token=' /tmp/opened.txt | tail -1 | sed 's/.*token=//')
UI="ws://127.0.0.1:23314/ui?token=$TOKEN"

echo "== switch starting at login off; the update downloads and waits for Install"
$E2E window "$UI" http://127.0.0.1:23314 startup-off | tail -1
[ ! -e "$AUTOSTART" ] || fail "startup-off left the autostart entry"
$E2E until "$UI" http://127.0.0.1:23314 'is ready.*asks for your password' 60 | tee /tmp/ready.json
grep -q '"updateAction": "install"' /tmp/ready.json || fail "no Install offered"
cmp -s "$H/.local/share/nous-relic-importer/update/nous-relic-importer" "$NEW" || fail "the staged file is not the new build"
sleep 3
kill -0 "$OLD_PID" || fail "the older copy installed by itself; on Linux it must wait"

echo "== Install from the window"
: > /tmp/opened.txt
$E2E window "$UI" http://127.0.0.1:23314 update-install | tail -1
wait_for "! kill -0 $OLD_PID 2>/dev/null" 200
wait_for "grep -q '^http://127.0.0.1:23314/?token=' /tmp/opened.txt" 300
cmp -s "$INSTALLED" "$NEW" || fail "the installed file is not the new build"
getcap "$INSTALLED" | grep -q cap_net_raw=ep || fail "the new file was not granted the capability: $(getcap "$INSTALLED")"
[ ! -e "$AUTOSTART" ] || fail "the update switched starting at login back on"
NEW_PID=$(pgrep -f "^$INSTALLED" | head -1)
grep -q '^CapEff:\s*0000000000000000' /proc/$NEW_PID/status || fail "the new copy kept a capability"
TOKEN=$(grep '^http://127.0.0.1:23314/?token=' /tmp/opened.txt | tail -1 | sed 's/.*token=//')
UI="ws://127.0.0.1:23314/ui?token=$TOKEN"
$E2E until "$UI" http://127.0.0.1:23314 '^Up to date' 60 | tee /tmp/current.json
grep -q "\"version\": \"$(grep -o '"version": "[^"]*"' "$FEED/latest.json" | cut -d'"' -f4)\"" /tmp/current.json || fail "not the new version"
echo "running the new version as pid $NEW_PID, granted, with starting at login still off"
grep -E 'handing over|update installed|started|up to date' "$H/.local/state/nous-relic-importer/sync.log"

echo "== quit, then uninstall"
$E2E window "$UI" http://127.0.0.1:23314 quit | tail -1
wait_for "! kill -0 $NEW_PID 2>/dev/null"
su tester -c "$INSTALLED --uninstall" || true
[ ! -e "$H/.local/share/nous-relic-importer" ] || fail "uninstall left the program folder"
echo "PASS"
