#!/bin/sh
# Installs niri-computer-use from a GitHub release: the binary to ~/.local/bin and the
# agent skill to ~/.local/share/niri-computer-use, linked from ~/.claude/skills and
# ~/.agents/skills.
#
# NCU_VERSION       the release to install, such as 0.1.0; the latest one by default
# NCU_BIN_DIR       where the binary goes; ~/.local/bin by default
# NCU_RELEASES_URL  where the releases are; the GitHub repository's by default
set -eu

releases=${NCU_RELEASES_URL:-https://github.com/ayagmar/niri-computer-use/releases}
bin_dir=${NCU_BIN_DIR:-${HOME}/.local/bin}
data_dir=${HOME}/.local/share/niri-computer-use

fail() {
	echo "install.sh: $*" >&2
	exit 1
}

fetch() {
	curl --fail --silent --show-error --location --connect-timeout 15 --max-time 300 "$@"
}

for tool in curl tar sha256sum uname mktemp; do
	command -v "${tool}" >/dev/null || fail "needs ${tool}, which isn't on PATH"
done

os=$(uname -s)
[ "${os}" = Linux ] || fail "niri-computer-use runs on Linux only, not ${os}"
arch=$(uname -m)
case ${arch} in
x86_64 | aarch64) ;;
*) fail "there is no release build for ${arch}, only x86_64 and aarch64; build it from source instead" ;;
esac

if [ -n "${NCU_VERSION:-}" ]; then
	version=${NCU_VERSION#v}
else
	latest=$(fetch --output /dev/null --write-out '%{url_effective}' "${releases}/latest")
	case ${latest} in
	*/tag/v*) version=${latest##*/tag/v} ;;
	*) fail "found no release at ${releases}/latest (ended at ${latest})" ;;
	esac
fi

name=niri-computer-use-${version}-${arch}-linux
tmp=$(mktemp -d)
trap 'rm -rf "${tmp}"' EXIT
trap 'exit 1' HUP INT TERM

echo "Downloading ${name}.tar.gz"
fetch --output "${tmp}/${name}.tar.gz" "${releases}/download/v${version}/${name}.tar.gz"
fetch --output "${tmp}/${name}.tar.gz.sha256" "${releases}/download/v${version}/${name}.tar.gz.sha256"
(cd "${tmp}" && sha256sum -c "${name}.tar.gz.sha256") ||
	fail "${name}.tar.gz doesn't match its .sha256; nothing was installed"
tar -xzf "${tmp}/${name}.tar.gz" -C "${tmp}"

# A binary this system can't run, such as one needing a newer glibc, stops here.
installed_version=$("${tmp}/${name}/niri-computer-use" --version) ||
	fail "the downloaded binary doesn't run here; nothing was installed"

# A new file moved into place, so a running server keeps the binary it started with.
mkdir -p "${bin_dir}"
cp "${tmp}/${name}/niri-computer-use" "${bin_dir}/.niri-computer-use.new"
chmod 755 "${bin_dir}/.niri-computer-use.new"
mv -f "${bin_dir}/.niri-computer-use.new" "${bin_dir}/niri-computer-use"

skill=${data_dir}/skills/niri-computer-use
mkdir -p "${data_dir}/skills"
rm -rf "${skill}"
cp -R "${tmp}/${name}/skills/niri-computer-use" "${skill}"
for skills_dir in "${HOME}/.claude/skills" "${HOME}/.agents/skills"; do
	link=${skills_dir}/niri-computer-use
	mkdir -p "${skills_dir}"
	if [ -L "${link}" ]; then
		rm "${link}"
	elif [ -e "${link}" ]; then
		echo "Left ${link} as it is: it isn't a link. The skill is in ${skill}."
		continue
	fi
	ln -s "${skill}" "${link}"
done

cat <<EOF

Installed ${installed_version} to ${bin_dir}/niri-computer-use and its skill to ${skill}.

Next:
  1. Check it from a shell inside your niri session; niri.compat should be "ok":
       ${bin_dir}/niri-computer-use status
  2. Register it in your agent, for Claude Code:
       claude mcp add --scope user niri-computer-use -- ${bin_dir}/niri-computer-use serve
     Other clients: https://ayagmar.github.io/niri-computer-use/start/clients/
  3. Bind the stop key inside the binds block of your niri config:
       Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true hotkey-overlay-title="Stop the AI agent (niri-computer-use)" { spawn "${bin_dir}/niri-computer-use" "stop"; }
EOF
