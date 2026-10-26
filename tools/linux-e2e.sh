#!/bin/bash
# The Linux app end to end, inside tools/linux.Dockerfile's image, as root:
#   tools/linux-e2e.sh <built binary> <recorded login pcap>
# A normal user runs the downloaded file once: it installs itself, gets the capture capability through a
# stand-in pkexec (sudo, no prompt), and starts the installed copy. Then the recorded PearlSR login is replayed
# onto loopback, and the site's socket and the window page must both have the account.
set -euo pipefail
BIN=$1
PCAP=$2
HERE=$(cd "$(dirname "$0")" && pwd)
E2E="python3 $HERE/linux_e2e.py"
fail() { echo "FAIL: $*"; exit 1; }
wait_for() { for _ in $(seq 1 100); do if eval "$1"; then return 0; fi; sleep 0.1; done; fail "timed out: $1"; }

id tester >/dev/null 2>&1 || useradd -m tester
echo 'tester ALL=(root) NOPASSWD: ALL' > /etc/sudoers.d/tester
# Stand-ins for the desktop: pkexec grants without a prompt, xdg-open records what the app asked to open.
printf '#!/bin/sh\nexec sudo -n "$@"\n' > /usr/local/bin/pkexec
printf '#!/bin/sh\necho "$1" >> /tmp/opened.txt\n' > /usr/local/bin/xdg-open
chmod 755 /usr/local/bin/pkexec /usr/local/bin/xdg-open
rm -f /tmp/opened.txt
install -m 755 "$BIN" /tmp/nous-relic-importer

echo "== first run: install, grant, hand over to the installed copy"
su tester -c /tmp/nous-relic-importer
H=/home/tester
INSTALLED=$H/.local/share/nous-relic-importer/nous-relic-importer
wait_for "$E2E site ws://127.0.0.1:23314/ws http://localhost:5173 >/dev/null 2>&1 || ss -ltn | grep -q 127.0.0.1:23314"
getcap "$INSTALLED" | grep -q cap_net_raw=ep || fail "no capability on the installed copy: $(getcap "$INSTALLED")"
for f in .local/share/applications/nous-relic-importer.desktop .config/autostart/nous-relic-importer.desktop \
         .local/share/icons/hicolor/256x256/apps/nous-relic-importer.png; do
  [ -f "$H/$f" ] || fail "missing $f"
done
grep -q -- '--background' "$H/.config/autostart/nous-relic-importer.desktop" || fail "autostart lacks --background"
PID=$(pgrep -f "^$INSTALLED" | head -1)
grep -E '^Cap(Prm|Eff):' /proc/$PID/status
grep -q '^CapEff:\s*0000000000000000' /proc/$PID/status || fail "the running app kept a capability"
echo "installed, granted, running as pid $PID with no capabilities left"

echo "== the recorded login, replayed onto loopback"
$E2E inject "$PCAP"
wait_for "$E2E site ws://127.0.0.1:23314/ws http://localhost:5173 2>/dev/null | grep -q '\"relics\": 558'"
$E2E site ws://127.0.0.1:23314/ws http://localhost:5173
$E2E refused ws://127.0.0.1:23314/ws https://evil.test || fail "another site's page was let in"
wait_for "[ -s $H/.local/state/nous-relic-importer/last-import.json ]"

echo "== starting it again shows the window"
su tester -c "$INSTALLED"
wait_for "grep -q '^http://127.0.0.1:23314/?token=' /tmp/opened.txt"
cat /tmp/opened.txt
PAGE=$(grep '^http://127.0.0.1:23314/?token=' /tmp/opened.txt | tail -1)
TOKEN=${PAGE#*token=}
$E2E page "$PAGE" | tee /tmp/page.json
grep -q '"status": 200, "page": true, "no_referrer": true' /tmp/page.json || fail "the window page"
$E2E page "http://127.0.0.1:23314/?token=wrong" | grep -q '"status": 404' || fail "a wrong token got the page"
$E2E page "$PAGE" "rebound.test:23314" | grep -q '"status": 404' || fail "another host name got the page"
$E2E refused "ws://127.0.0.1:23314/ui?token=$TOKEN" https://evil.test || fail "another page reached the window socket"

echo "== the window's controls"
$E2E window "ws://127.0.0.1:23314/ui?token=$TOKEN" http://127.0.0.1:23314 startup-off startup-on | tee /tmp/window.json
grep -q '"headline": "Synced 558 relics"' /tmp/window.json || fail "the window's headline"
grep -q '"startup": false' /tmp/window.json || fail "startup-off"
[ -f "$H/.config/autostart/nous-relic-importer.desktop" ] || fail "startup-on did not put the autostart entry back"

echo "== Auto import off, then on: an open page is told it is paused, then starts over from the account"
$E2E events ws://127.0.0.1:23314/ws http://localhost:5173 5 > /tmp/events.txt &
EVENTS=$!
sleep 1
$E2E window "ws://127.0.0.1:23314/ui?token=$TOKEN" http://127.0.0.1:23314 sending-off sending-on | tee /tmp/sending.json
wait $EVENTS
[ "$(tr '\n' ' ' < /tmp/events.txt)" = "InitialScan:558 Paused InitialScan:558 " ] || fail "the page saw: $(tr '\n' ' ' < /tmp/events.txt)"
grep -q '"sending": false.*Auto import is off\|Auto import is off.*"sending": false' /tmp/sending.json \
  || fail "the window does not say sending is off"
grep -q '"sendToSite":true' "$H/.local/state/nous-relic-importer/settings.json" || fail "the switch was not kept for the next start"
echo "the page saw: $(tr '\n' ' ' < /tmp/events.txt)"

if [ "${HOLD:-0}" != 0 ]; then
  echo "== holding for $HOLD s: the window page is on port 23315 too"
  echo "$PAGE" > /tmp/page-url.txt
  socat TCP-LISTEN:23315,fork,reuseaddr TCP:127.0.0.1:23314 &
  sleep "$HOLD"
fi

echo "== uninstall while it runs: the running copy quits first"
su tester -c "$INSTALLED --uninstall" || true
wait_for "! kill -0 $PID 2>/dev/null"
for f in .local/share/nous-relic-importer .local/state/nous-relic-importer .local/share/applications/nous-relic-importer.desktop \
         .config/autostart/nous-relic-importer.desktop; do
  [ ! -e "$H/$f" ] || fail "uninstall left $f"
done
echo "PASS"
