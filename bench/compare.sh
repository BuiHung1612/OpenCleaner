#!/bin/zsh
# Head-to-head: the Rust engine (oce) against the legacy mo engine, on this Mac.
# Read-only: every command is a scan or a preview; nothing is removed.
# Usage: bench/compare.sh [runs]   (default 2)  -> markdown table on stdout
# Needs: cargo (builds oce), and the mo engine (legacy/mo/fetch-engine.sh builds it).
# Close busy apps first: results swing with machine load (the load average is printed).
set -euo pipefail
cd "${0:A:h}/.."
RUNS=${1:-2}

(cd engine && cargo build -q --release --bin oce)
OCE=$PWD/engine/target/release/oce
MO=$PWD/macos/$(legacy/mo/fetch-engine.sh)/mole

# label | mo command | oce command
CASES=(
  "System Junk preview|$MO clean --dry-run|$OCE scan"
  "Space Lens: startup disk|$MO analyze --json|$OCE space"
  "Space Lens: ~/Library|$MO analyze --json $HOME/Library|$OCE space $HOME/Library"
  "Space Lens: /Applications|$MO analyze --json /Applications|$OCE space /Applications"
  "App list with sizes|$MO uninstall --list|$OCE apps"
  "Dashboard reading|$MO status --json|$OCE status"
  "Installers|$MO installer --dry-run|$OCE installers"
)

# prints "<seconds> <peak MiB>" for one run
measure() {
  local report=$(mktemp)
  /usr/bin/time -l -o $report zsh -c "$1" </dev/null >/dev/null 2>&1 || true
  local real=$(awk '/ real /{print $1; exit}' $report)
  local rss=$(awk '/maximum resident set size/{printf "%.0f", $1/1048576; exit}' $report)
  rm -f $report
  print "${real:-0} ${rss:-0}"
}

best() { print -l -- "$@" | sort -n | head -1 }

print "OpenCleaner engine benchmark — $(date '+%Y-%m-%d %H:%M'), $(sysctl -n hw.model), $(sysctl -n machdep.cpu.brand_string), $(sysctl -n hw.ncpu) cores"
print "Load average at start: $(sysctl -n vm.loadavg)  ·  best of $RUNS runs, alternating mo / Rust"
print
print "| Task | mo (legacy) | Rust engine | Speed-up | Peak RAM mo → Rust |"
print "|---|---:|---:|---:|---:|"
for c in $CASES; do
  label=${c%%|*}; rest=${c#*|}; mo_cmd=${rest%%|*}; oce_cmd=${rest#*|}
  mo_t=(); oce_t=(); mo_m=(); oce_m=()
  for i in $(seq $RUNS); do
    r=($(measure "$mo_cmd")); mo_t+=(${r[1]}); mo_m+=(${r[2]})
    r=($(measure "$oce_cmd")); oce_t+=(${r[1]}); oce_m+=(${r[2]})
  done
  a=$(best $mo_t); b=$(best $oce_t)
  speed=$(awk -v a=$a -v b=$b 'BEGIN { if (b > 0) printf "%.1f×", a / b; else print "—" }')
  print "| $label | ${a} s | ${b} s | $speed | $(best $mo_m) → $(best $oce_m) MiB |"
done
print
print "Peak RAM is the largest single process of the run (/usr/bin/time -l), not the sum: mo runs many processes at once."
