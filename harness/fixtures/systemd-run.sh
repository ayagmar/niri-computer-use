#!/bin/bash
# Stands in for systemd-run in `make nested-desk`'s interrupted starts. `desk start` hands
# it the desk's lock as its stdin. It resolves the supervisor's binary at once, since
# `desk start` names it by its own /proc entry, says it has started, then:
#
# - DESK_FAKE_MODE=real: runs the real systemd-run, DESK_FAKE_REAL, with the same scope,
#   on a shell that sleeps DESK_FAKE_DELAY seconds in the scope, holding the lock, then
#   becomes the supervisor: a supervisor whose start is late.
# - DESK_FAKE_MODE=stray: after DESK_FAKE_DELAY seconds, starts only a stray process in the
#   desk's scope and exits, as a supervisor that died before it made the desk's directory
#   would leave it. The stray's PID goes to DESK_FAKE_DIR/stray.
set -euo pipefail
options=()
while [[ $# -gt 0 && $1 != -- ]]; do
	options+=("$1")
	shift
done
shift
program=$(readlink -f "$1")
shift
: >"${DESK_FAKE_DIR:?}/started"
case "${DESK_FAKE_MODE:?}" in
real)
	# shellcheck disable=SC2016 # the inner shell expands $0 and $@
	exec "${DESK_FAKE_REAL:?}" "${options[@]}" -- \
		sh -c 'sleep "$0"; exec "$@"' "${DESK_FAKE_DELAY:?}" "${program}" "$@"
	;;
stray)
	unit=
	for option in "${options[@]}"; do
		if [[ ${option} == --unit=* ]]; then
			unit=${option#--unit=}
		fi
	done
	sleep "${DESK_FAKE_DELAY:?}"
	# shellcheck disable=SC2016 # the inner shell expands $! and $1
	"${DESK_FAKE_REAL:?}" --user --scope --collect --quiet --unit="${unit}" -- \
		sh -c 'sleep 300 </dev/null >/dev/null 2>&1 & echo "$!" >"$1"' sh "${DESK_FAKE_DIR:?}/stray" </dev/null
	exit 1
	;;
*) exit 2 ;;
esac
