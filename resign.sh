#!/bin/bash

PLAYER="/Applications/Roblox.app/Contents/MacOS/RobloxPlayer"
PLIST="/tmp/fps-unlocker-resign.plist"
LOG="/tmp/fps-unlocker-resign.log"

cleanup() {
  [ -n "${KEEPALIVE:-}" ] && kill "$KEEPALIVE" 2>/dev/null
  rm -f "$PLIST"
}
trap cleanup EXIT

fail() {
  printf '\n%s\n' "$1"
  [ -s "$LOG" ] && { printf '\n'; tail -n 3 "$LOG"; }
  exit 1
}

[ -x "$PLAYER" ] || fail "Roblox is not installed."

echo "Resigning Roblox requires administrator privileges. Please enter the password you use to log into your Mac."
sudo -v || fail "authentication failed"

( while true; do sudo -n true; sleep 30; kill -0 "$$" 2>/dev/null || exit; done ) &
KEEPALIVE=$!

exec 2>>"$LOG"

killall -x RobloxPlayer 2>/dev/null

cat > "$PLIST" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>com.apple.security.get-task-allow</key><true/></dict></plist>
PLIST

sudo -n xattr -cr /Applications/Roblox.app
sudo -n /usr/bin/codesign --force --sign - --entitlements "$PLIST" "$PLAYER" || fail "couldnt sign roblox"

entitlements=$(/usr/bin/codesign -d --entitlements :- "$PLAYER" 2>&1) || fail "couldnt read entitlments"
printf '%s' "$entitlements" | grep -q "com.apple.security.get-task-allow" || fail "get-task-allow is missing"

printf '\nRoblox resigned.\n'
