#!/usr/bin/env bash
# Records one output for a demo: `grim` frames, cursor included, at about 30 a second, into
# RUN/frames/<capture time in ns>.jpg, until RUN/STOP exists. It only reads the screen.
#
#   scripts/demo-record.sh RUN [OUTPUT]
#
# OUTPUT defaults to the focused output's name as niri reports it.
set -euo pipefail

run=${1:?usage: demo-record.sh RUN [OUTPUT]}
output=${2:-$(timeout 5 niri msg --json focused-output | jq -r .name)}
frame_ns=33000000

mkdir -p "${run}/frames"
date -u +%Y-%m-%dT%H:%M:%S >"${run}/since"
while [[ ! -e "${run}/STOP" ]]; do
	start=$(date +%s%N)
	timeout 2 grim -c -o "${output}" -t jpeg -q 88 "${run}/frames/${start}.jpg"
	left=$((frame_ns - ($(date +%s%N) - start)))
	if ((left > 0)); then
		sleep "0.$(printf '%09d' "${left}")"
	fi
done
