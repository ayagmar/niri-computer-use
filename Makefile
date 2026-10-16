.PHONY: check check-static test test-shared stall stall-standalone stall-shared perf lint coverage inspect inspect-check nested nested-control nested-actions nested-input nested-shell nested-a11y nested-engine nested-measure nested-latency nested-chromium nested-chromium-a11y nested-big nested-compat niri-matrix nested-upgrade nested-eval nested-demo sitting host-capture

SCALE ?= 1
NESTED_FLAGS = $(if $(VISIBLE),--visible) $(if $(SHARED),--shared) $(if $(NIRI),--niri $(NIRI))
NIRI_VERSIONS ?= v25.08 v25.11 v26.04 main

# CI runs each of check's parts as a job of its own.
check: check-static test test-shared

check-static:
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
	cargo deny check
	cargo machete
	typos
	git ls-files -z '*.sh' .githooks | xargs -0 -r shellcheck -o all

# Every test target, with the protocol suite in standalone mode.
test:
	cargo test --all-targets --locked

test-shared:
	NCU_PROTOCOL_MODE=shared cargo test --test protocol --locked

# The stall explorer's full matrix; make check explores one tool of each kind.
stall: stall-standalone stall-shared

stall-standalone:
	NCU_STALL=full cargo test --test protocol --locked stall::

stall-shared:
	NCU_STALL=full NCU_PROTOCOL_MODE=shared cargo test --test protocol --locked stall::

# Unit tests that also check how long their calls take, which a loaded machine can miss.
perf:
	NCU_PERF=1 cargo test --bins --locked

lint:
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings

coverage:
	cargo llvm-cov --all-targets --workspace

inspect:
	scripts/inspector.sh web

inspect-check:
	scripts/inspector.sh check

nested:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --scale $(SCALE) $(if $(NOCTALIA),--noctalia)

nested-control:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --control

nested-actions:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --actions

nested-input:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --scale $(SCALE) --input

nested-shell:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --shell

nested-a11y:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --scale $(SCALE) $(if $(SSD),--ssd) --a11y

nested-engine:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --scale $(SCALE) --engine

# Release builds; SERVER=<path> measures another build, such as an older one.
nested-measure:
	cargo build --locked --release -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --measure --server $(or $(SERVER),target/release/niri-computer-use)

# Each action tool's round trip, release builds; SERVER=<path> measures another build.
nested-latency:
	cargo build --locked --release -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --latency --server $(or $(SERVER),target/release/niri-computer-use)

# Typed and pasted text read back from a Chromium text area; NCU_CHROMIUM=<path> picks the browser.
nested-chromium:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --chromium

nested-chromium-a11y:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --scale $(SCALE) $(if $(SSD),--ssd) --chromium-a11y

# Windows with one big container, from fixtures/big.py, read whole by a release build.
nested-big:
	cargo build --locked --release -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --big --server target/release/niri-computer-use

# What the server does on one niri version: make nested-compat NIRI=<path to a niri binary>
nested-compat:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --compat

# Each niri in NIRI_VERSIONS built from source, then checked against this checkout's server.
niri-matrix:
	cargo build --locked -p niri-computer-use
	mkdir -p target/niri-matrix
	scripts/niri-build.sh $(NIRI_VERSIONS) > target/niri-matrix/builds.txt
	cargo run --locked -p harness -- matrix target/niri-matrix/builds.txt docs/results/niri-matrix.md

# An upgrade from each published release to this release build, installed with install.sh
# into a throwaway HOME: make nested-upgrade UPGRADE_FROM=0.1.1 picks one.
UPGRADE_FROM ?= 0.1.0 0.1.1 0.1.2
nested-upgrade:
	cargo build --locked --release -p niri-computer-use
	for from in $(UPGRADE_FROM); do \
		cargo run --locked -p harness -- run $(NESTED_FLAGS) --upgrade $$from --server target/release/niri-computer-use || exit 1; \
	done

# One skill eval in the nested niri: make nested-eval SCENARIO=compose-message SKILL=skills/niri-computer-use MODEL=sonnet
nested-eval:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --eval $(SCENARIO) --skill $(SKILL) --model $(MODEL)

host-capture:
	cargo run --locked -p harness -- host-capture $(OUTPUT)

sitting:
	cargo run --locked -p harness -- run --scale 1.5 --sitting

# A demo recording in the nested niri: make nested-demo SCENE=errand; then scripts/demo-encode.py
nested-demo:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --demo $(SCENE)
