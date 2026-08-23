#!/bin/bash

BASE="https://github.com/imeowforcash/fps-unlocker/releases/latest/download"
APP_TAR="/tmp/fps-unlocker-app.tar.gz"
APP_STATE="/tmp/fps-unlocker-app.state"
LOG="/tmp/fps-unlocker-install.log"
ERR="/tmp/fps-unlocker-error"

cleanup() {
  [ -n "${KEEPALIVE:-}" ] && kill "$KEEPALIVE" 2>/dev/null
  [ -n "${AT:-}" ] && kill "$AT" 2>/dev/null
  rm -rf "$APP_TAR" "$APP_STATE" "$ERR" /tmp/fps-unlocker-app 2>/dev/null
}
trap cleanup EXIT

fail() {
  local msg=${1:-}
  printf '\nFPS Unlocker installation failed: %s\n' "${msg:-unknown error}"
  [ -s "$LOG" ] && { printf '\n'; tail -n 3 "$LOG"; }
  exit 1
}

die() {
  echo "$1" > "$ERR"
  exit 1
}

bar() {
  local pct=${1:-0} width=28 filled i s=""
  [[ "$pct" =~ ^[0-9]+$ ]] || pct=0
  [ "$pct" -gt 100 ] && pct=100
  filled=$(( pct * width / 100 ))
  for ((i=0;i<filled;i++)); do s+="█"; done
  if (( filled < width )); then s+="▓"; ((i++)); fi
  for ((;i<width;i++)); do s+="░"; done
  printf '%s' "$s"
}

render() {
  local pct
  pct=$(cat "$APP_STATE" 2>/dev/null || echo 0)
  printf '\033[2A'
  printf '\r\033[KInstalling FPS Unlocker\n'
  printf '\r\033[K%s\n' "$(bar "$pct")"
}

size_of() {
  curl -fsSLI "$1" 2>/dev/null | awk 'tolower($1) ~ /content-length/ {v=$2} END{gsub(/\r/,"",v); print v+0}'
}

install_task() {
  local url="$1" total="$2" cur dl app
  curl -fL -s --retry 5 --retry-all-errors --retry-delay 2 -C - "$url" -o "$APP_TAR" &
  dl=$!
  while kill -0 "$dl" 2>/dev/null; do
    cur=$(stat -f%z "$APP_TAR" 2>/dev/null || echo 0)
    [ "$total" -gt 0 ] && echo $(( cur * 90 / total )) > "$APP_STATE"
    sleep 0.2
  done
  wait "$dl" || die "download failed"
  echo 92 > "$APP_STATE"
  rm -rf /tmp/fps-unlocker-app && mkdir -p /tmp/fps-unlocker-app
  tar -xzf "$APP_TAR" -C /tmp/fps-unlocker-app || die "could not unpack FPS Unlocker"
  app=$(find /tmp/fps-unlocker-app -maxdepth 1 -name '*.app' | head -1)
  [ -n "$app" ] || die "the FPS Unlocker download was empty"
  killall "$(basename "$app" .app)" fps-unlocker 2>/dev/null
  sudo -n rm -rf "/Applications/$(basename "$app")" "/Applications/fps-unlocker.app" || die "could not remove the old FPS Unlocker"
  sudo -n mv "$app" /Applications/ || die "could not install FPS Unlocker"
  sudo -n xattr -cr "/Applications/$(basename "$app")" || die "could not finalize FPS Unlocker"
  echo 100 > "$APP_STATE"
}

case "$(uname -m)" in
  arm64) file=fps-unlocker-aarch64.app.tar.gz ;;
  x86_64) file=fps-unlocker-x86_64.app.tar.gz ;;
  *) echo "Your Mac is incompatible with FPS Unlocker."; exit 1 ;;
esac

app_url="$BASE/$file"

echo "FPS Unlocker installation requires administrator privileges. Please enter the password you use to log into your Mac."
sudo -v || fail "authentication failed"

( while true; do sudo -n true; sleep 30; kill -0 "$$" 2>/dev/null || exit; done ) &
KEEPALIVE=$!

exec 2>>"$LOG"

app_total=$(size_of "$app_url")
echo 0 > "$APP_STATE"

printf '\nStarting Installation.\n\n'
printf 'Installing FPS Unlocker\n%s\n' "$(bar 0)"

install_task "$app_url" "${app_total:-0}" &
AT=$!

while kill -0 "$AT" 2>/dev/null; do
  render
  sleep 0.2
done
render

wait "$AT" || fail "$(cat "$ERR" 2>/dev/null)"

printf '\nFPS Unlocker installation complete.\n'
