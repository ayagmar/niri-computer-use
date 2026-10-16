#!/usr/bin/env bash
# Builds niri at git refs, such as v26.04 or main, into a cache for the compatibility
# matrix. Nothing is installed and no niri binary is run.
#
#   scripts/niri-build.sh REF...
#
# For each REF it prints one line: `REF COMMIT PATH`, where PATH is the niri binary built
# from that commit, or `REF COMMIT failed: REASON` when the build failed, with the build's
# first error line; the build's output is in logs/COMMIT.log. A commit already built is not
# built again. `main` and other branch names are fetched from the remote first; a tag or a
# commit hash is fixed. It exits 1 only when a REF names no commit or a prerequisite is
# missing.
#
#   NIRI_MATRIX_DIR  the cache (default ${XDG_CACHE_HOME:-~/.cache}/niri-computer-use/niri-matrix):
#                    src/ a niri clone, work/COMMIT/ its worktrees, target/ the shared
#                    Cargo target directory, bin/COMMIT/niri the builds, logs/ their output
#   NIRI_REPO        where to clone niri from (default https://github.com/YaLTeR/niri.git)
#
# Missing build prerequisites are listed, not installed.
set -euo pipefail

readonly FETCH_SECONDS=600
readonly BUILD_SECONDS=3600
# What niri's Building section asks for, as pkg-config modules.
readonly MODULES=(libudev gbm xkbcommon egl wayland-server libinput dbus-1 libsystemd libseat
	libpipewire-0.3 pango pangocairo cairo-gobject libdisplay-info)

if (($# == 0)); then
	echo "usage: scripts/niri-build.sh REF..." >&2
	exit 2
fi

dir=${NIRI_MATRIX_DIR:-${XDG_CACHE_HOME:-${HOME}/.cache}/niri-computer-use/niri-matrix}
repo=${NIRI_REPO:-https://github.com/YaLTeR/niri.git}

missing=()
for program in git cargo clang pkg-config timeout; do
	command -v "${program}" >/dev/null || missing+=("${program}")
done
if command -v pkg-config >/dev/null; then
	for module in "${MODULES[@]}"; do
		pkg-config --exists "${module}" || missing+=("pkg-config module ${module}")
	done
fi
if ((${#missing[@]} > 0)); then
	printf 'niri-build: missing build prerequisite: %s\n' "${missing[@]}" >&2
	exit 1
fi

mkdir -p "${dir}"
if [[ ! -d "${dir}/src/.git" ]]; then
	timeout "${FETCH_SECONDS}" git clone --quiet "${repo}" "${dir}/src" >&2
fi
timeout "${FETCH_SECONDS}" git -C "${dir}/src" fetch --quiet --tags --prune origin >&2

# A branch on the remote, then a tag, then anything else git can name.
resolve() {
	local ref=$1 name
	for name in "refs/remotes/origin/${ref}" "refs/tags/${ref}" "${ref}"; do
		if git -C "${dir}/src" rev-parse --verify --quiet "${name}^{commit}"; then
			return
		fi
	done
	echo "niri-build: ${ref} names no commit in ${dir}/src" >&2
	return 1
}

# Builds COMMIT unless it is built, and copies the binary in only once it is whole.
build() {
	local commit=$1 work="${dir}/work/$1" bin="${dir}/bin/$1"
	if [[ -x "${bin}/niri" ]]; then
		return
	fi
	if [[ ! -d "${work}" ]]; then
		git -C "${dir}/src" worktree add --quiet --detach "${work}" "${commit}"
	fi
	echo "niri-build: building ${commit}" >&2
	(cd "${work}" && CARGO_TARGET_DIR="${dir}/target" timeout "${BUILD_SECONDS}" \
		cargo build --release --locked)
	mkdir -p "${bin}"
	cp "${dir}/target/release/niri" "${bin}/niri.partial"
	mv "${bin}/niri.partial" "${bin}/niri"
}

mkdir -p "${dir}/logs"
for ref in "$@"; do
	commit=$(resolve "${ref}")
	log="${dir}/logs/${commit}.log"
	# A subshell, so that set -e still stops the build at its first failing step.
	set +e
	(set -e && build "${commit}") >"${log}" 2>&1
	status=$?
	set -e
	if ((status == 0)); then
		echo "${ref} ${commit} ${dir}/bin/${commit}/niri"
	else
		reason=$(grep -m 1 'was not found' "${log}" || grep -m 1 '^error' "${log}" || echo "exit ${status}")
		echo "niri-build: ${ref} (${commit}) failed; see ${log}" >&2
		echo "${ref} ${commit} failed: ${reason}"
	fi
done
