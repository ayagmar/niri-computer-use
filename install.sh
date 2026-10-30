#!/bin/sh
# Installs niri-computer-use from a GitHub release: the binary to ~/.local/bin, and the
# agent skill and the Noctalia plugin to ~/.local/share/niri-computer-use. The skill is
# linked from ~/.claude/skills and ~/.agents/skills, and the plugin from Noctalia's plugin
# directory when noctalia is on PATH.
#
# NCU_VERSION       the release to install, such as 0.1.0; the latest one by default
# NCU_BIN_DIR       where the binary goes; ~/.local/bin by default
# NCU_RELEASES_URL  where the releases are; the GitHub repository's by default
# NCU_REGISTER      1 registers the server in every agent found on PATH (claude, pi,
#                   codex, amp, copilot, gemini, qwen) without asking, 0 registers in
#                   none; unset asks per agent when the output is a terminal the script
#                   can open, and otherwise registers in none
set -eu

# The whole script is one brace group, which sh reads to its end before running any of it.
# Piped from curl, a download cut short runs nothing, and a program that reads stdin can't
# take the script's own lines.
{

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
status=0
installed_version=$("${tmp}/${name}/niri-computer-use" --version) || status=$?
[ "${status}" -eq 0 ] ||
	fail "the downloaded binary exited with status ${status} (its error, if any, is above), so nothing was installed. It needs glibc 2.39 or newer. If TMPDIR is mounted noexec, set TMPDIR to a directory programs can run from."

# The binary, the skill and the plugin are all copied beside where they go before any
# replaces anything, so a failed copy leaves the previous install as it was. Each then
# goes into place with a rename, so a running server keeps the binary it started with.
# A release before the plugin has none, and leaves an installed one as it is.
mkdir -p "${bin_dir}" "${data_dir}/skills" "${data_dir}/noctalia"
bin_dir=$(cd "${bin_dir}" && pwd)
binary=${bin_dir}/niri-computer-use
skill=${data_dir}/skills/niri-computer-use
plugin=${data_dir}/noctalia/niri-computer-use
staged_binary=${bin_dir}/.niri-computer-use.new
staged_skill=${data_dir}/skills/.niri-computer-use.new
staged_plugin=${data_dir}/noctalia/.niri-computer-use.new
trap 'rm -rf "${tmp}" "${staged_binary}" "${staged_skill}" "${staged_plugin}"' EXIT
rm -rf "${staged_binary}" "${staged_skill}" "${staged_plugin}"
has_plugin=no
[ -d "${tmp}/${name}/noctalia/niri-computer-use" ] && has_plugin=yes
if ! {
	cp "${tmp}/${name}/niri-computer-use" "${staged_binary}" &&
		chmod 755 "${staged_binary}" &&
		cp -R "${tmp}/${name}/skills/niri-computer-use" "${staged_skill}" &&
		{ [ "${has_plugin}" = no ] || cp -R "${tmp}/${name}/noctalia/niri-computer-use" "${staged_plugin}"; }
}; then
	fail "couldn't copy the new files into ${bin_dir} and ${data_dir}; what was installed before is unchanged"
fi
mv -f "${staged_binary}" "${binary}"
rm -rf "${skill}"
mv "${staged_skill}" "${skill}"
if [ "${has_plugin}" = yes ]; then
	rm -rf "${plugin}"
	mv "${staged_plugin}" "${plugin}"
fi

# $1 what the link points to, $2 the link, $3 what it is, as in "The skill is in".
link_to() {
	mkdir -p "$(dirname "$2")"
	if [ -L "$2" ]; then
		target=$(readlink "$2")
		[ "${target}" = "$1" ] || echo "Replaced $2, which pointed to ${target}."
		rm "$2"
	elif [ -e "$2" ]; then
		echo "Left $2 as it is: it isn't a link. $3 is in $1."
		return 0
	fi
	ln -s "$1" "$2"
}

for skills_dir in "${HOME}/.claude/skills" "${HOME}/.agents/skills"; do
	link_to "${skill}" "${skills_dir}/niri-computer-use" "The skill"
done
noctalia_plugins=${XDG_DATA_HOME:-${HOME}/.local/share}/noctalia/plugins
linked_plugin=no
if [ -d "${plugin}" ] && command -v noctalia >/dev/null; then
	link_to "${plugin}" "${noctalia_plugins}/niri-computer-use" "The Noctalia plugin"
	linked_plugin=yes
fi

# Registration in the agents on PATH, each through its own `mcp add`, so no config file
# is edited. Nothing here fails the install: an agent that refuses is reported with the
# command to run by hand.
case ${NCU_REGISTER:-} in
1) register=yes ;;
0) register=no ;;
'') if [ -t 1 ] && (: </dev/tty) 2>/dev/null; then register=ask; else register=no; fi ;;
*) fail "NCU_REGISTER must be 1 or 0, not ${NCU_REGISTER}" ;;
esac
registered=""
todo=""

# $1 the agent's name, $2 its program, $3 the command as shown, the rest the command.
try_register() {
	agent=$1
	program=$2
	shown=$3
	shift 3
	command -v "${program}" >/dev/null || return 0
	if [ "${register}" = ask ]; then
		printf 'Register it in %s now, with %s? [y/N] ' "${agent}" "${shown}" >/dev/tty
		read -r answer </dev/tty || answer=
		case ${answer} in
		y | Y | yes) ;;
		*) todo="${todo}       ${shown}
" && return 0 ;;
		esac
	elif [ "${register}" = no ]; then
		todo="${todo}       ${shown}
"
		return 0
	fi
	if [ "${program}" = claude ] && claude mcp get niri-computer-use >/dev/null 2>&1; then
		registered="${registered}     Already registered in ${agent}; left as it is.
"
		return 0
	fi
	if output=$("$@" 2>&1); then
		registered="${registered}     Registered in ${agent}.
"
	else
		last=$(printf '%s\n' "${output}" | tail -n 1)
		echo "Registering in ${agent} failed: ${last}" >&2
		todo="${todo}       ${shown}
"
	fi
}

try_register "Claude Code" claude "claude mcp add --scope user niri-computer-use -- \"${binary}\" serve" \
	claude mcp add --scope user niri-computer-use -- "${binary}" serve
try_register Pi pi "pi mcp add niri-computer-use --exposure direct -- \"${binary}\" serve" \
	pi mcp add niri-computer-use --exposure direct -- "${binary}" serve
try_register Codex codex "codex mcp add niri-computer-use -- \"${binary}\" serve" \
	codex mcp add niri-computer-use -- "${binary}" serve
try_register Amp amp "amp mcp add niri-computer-use -- \"${binary}\" serve" \
	amp mcp add niri-computer-use -- "${binary}" serve
try_register "GitHub Copilot CLI" copilot "copilot mcp add niri-computer-use -- \"${binary}\" serve" \
	copilot mcp add niri-computer-use -- "${binary}" serve
try_register "Gemini CLI" gemini "gemini mcp add --scope user niri-computer-use \"${binary}\" serve" \
	gemini mcp add --scope user niri-computer-use "${binary}" serve
try_register "Qwen Code" qwen "qwen mcp add --scope user niri-computer-use \"${binary}\" serve" \
	qwen mcp add --scope user niri-computer-use "${binary}" serve
if [ -z "${registered}${todo}" ]; then
	todo="       claude mcp add --scope user niri-computer-use -- \"${binary}\" serve
"
fi

cat <<EOF

Installed ${installed_version} to ${binary} and its skill to ${skill}.

Next:
  1. Register it in your agent:
EOF
printf '%s' "${registered}"
if [ -n "${todo}" ]; then
	printf '%s' "${todo}"
fi
cat <<EOF
     Other clients: https://ayagmar.github.io/niri-computer-use/start/clients/
  2. Bind the stop key inside the binds block of your niri config:
       Mod+Shift+Escape allow-inhibiting=false allow-when-locked=true hotkey-overlay-title="Stop the AI agent (niri-computer-use)" { spawn "${binary}" "stop"; }
EOF
check=3
if [ "${linked_plugin}" = yes ]; then
	cat <<EOF
  3. See which agent drives your desktop in Noctalia's bar: enable the plugin, then add
     its widget to a bar and its glow to the desktop in Noctalia's settings:
       noctalia msg plugins enable ayagmar/niri-computer-use
EOF
	check=4
fi
cat <<EOF
  ${check}. Check it from a shell inside your niri session; every line should be a ✓:
       "${binary}" status --check
EOF

# Servers started before this install keep running the build it replaced. The shared
# engine of that build serves them until the last one exits or, when it is newer than
# 0.1.5, until it hands over to a client of the installed build.
old=""
for exe in /proc/[0-9]*/exe; do
	running=$(readlink "${exe}" 2>/dev/null) || continue
	[ "${running}" = "${binary} (deleted)" ] || continue
	dir=${exe%/exe}
	subcommand=$(tr '\0' '\n' 2>/dev/null <"${dir}/cmdline" | sed -n 2p) || continue
	[ "${subcommand}" = serve ] || continue
	parent=$(sed -n 's/^PPid:[[:space:]]*//p' "${dir}/status" 2>/dev/null)
	agent=$(cat "/proc/${parent}/comm" 2>/dev/null) || agent=unknown
	old="${old}       PID ${dir#/proc/}, started by ${agent}
"
done
if [ -n "${old}" ]; then
	cat <<EOF

These servers still run the build this install replaced. Restart their agents, or
reconnect the MCP server (/mcp in Claude Code), to use ${installed_version}. Until then
one may get engine_lost once, and then errors saying to restart it:
EOF
	printf '%s' "${old}"
fi

}
