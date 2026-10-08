#!/usr/bin/env bash
#
# Acceptance measurement for the matching forms: prefix, substring, wildcard,
# character class and suffix, measured in one sitting on one machine.
#
# Unlike run-acceptance.sh there is no reference binary here. mkp224o has no
# substring, wildcard or suffix of its own — only PCRE2, which is a different
# thing measured in docs/analysis/filter-scaling.md. The prefix row is the
# anchor instead: it is the form both implementations share, and its absolute
# figure says what state the machine was in while the rest were taken.
#
# Every filter is long enough that no hit lands during a run, so the matcher is
# what is being measured and not the writer.
#
# Usage: run-form-acceptance.sh [samples]

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OURS="$ROOT/target/release/onion-gen"
SAMPLES="${1:-5}"
INTERVAL=2
THREADS="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 8)"

[ -x "$OURS" ] || { echo "build it first: cargo build --release" >&2; exit 2; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

power="unknown"
if command -v pmset >/dev/null; then
	pmset -g batt 2>/dev/null | head -1 | grep -qi "AC Power" && power="AC" || power="battery"
fi

# `timeout` returns non-zero when it stops a run, which is the normal way every
# measurement here ends. Errexit must not read that as a failure.
arith="$(timeout 5 "$OURS" --arithmetic auto -n 1 -x -d "$work/probe" -F abcd 2>&1 \
	| grep -m1 "field arithmetic" | sed 's/^onion-gen: field arithmetic: //' || true)"
rm -rf "$work/probe"

cat <<META
# generated_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
# arch=$(uname -m) os=$(uname -s) osver=$(uname -r) power=$power
# arithmetic=${arith:-?}
# threads=$THREADS interval=${INTERVAL}s samples=$SAMPLES
# note: one filter per row, long enough that no hit lands; all rows in one sitting
META
echo "form,filter,calc_per_sec_median,calc_per_sec_min,calc_per_sec_max,samples"

measure() { # form filter
	local rates=() out
	for _ in $(seq 1 "$SAMPLES"); do
		rm -rf "$work/out"
		mkdir -p "$work/out"
		out="$(timeout $((INTERVAL * 6)) "$OURS" -t "$THREADS" -s "$INTERVAL" -x \
			-d "$work/out" -F "$2" 2>&1 | grep -o 'calc/sec:[0-9.]*' | cut -d: -f2 \
			| sort -g | tail -1 || true)"
		[ -n "$out" ] && rates+=("${out%.*}") || true
	done
	if [ ${#rates[@]} -eq 0 ]; then
		printf '%s,%s,,,,0\n' "$1" "$2"
		return
	fi
	printf '%s\n' "${rates[@]}" | sort -n | awk -v form="$1" -v filter="$2" '
		{ v[NR] = $1 }
		END {
			if (NR == 0) { printf "%s,%s,,,,0\n", form, filter; exit }
			median = (NR % 2) ? v[(NR + 1) / 2] : int((v[NR / 2] + v[NR / 2 + 1]) / 2)
			printf "%s,%s,%d,%d,%d,%d\n", form, filter, median, v[1], v[NR], NR
		}'
}

measure prefix    abcdefghij
measure substring contains:abcdefghij
measure wildcard  'a?cdefghij'
measure class     'a[b-e]cdefghij'
measure suffix    suffix:abcdefghijkzad
# The sixth form. Three rows because the three paths cost different things and
# one number for "regex" would average them into a figure describing none: the
# first is guarded on the bits, the second reaches the checksum but is still
# guarded there, and the third has nothing to guard it at all.
measure regex-bits    'regex:^abcdefghij'
measure regex-address 'regex:^abcdefghij.*'
measure regex-open    'regex:^[bcdfghjklmnp]{10}'
