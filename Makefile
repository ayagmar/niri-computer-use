.PHONY: check lint coverage inspect inspect-check nested nested-control nested-actions nested-input nested-shell nested-a11y nested-engine nested-measure nested-eval sitting host-capture

SCALE ?= 1
NESTED_FLAGS = $(if $(VISIBLE),--visible) $(if $(SHARED),--shared)

check:
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
	cargo test --all-targets --locked
	NCU_PROTOCOL_MODE=shared cargo test --test protocol --locked
	cargo deny check
	cargo machete
	typos
	git ls-files -z '*.sh' .githooks | xargs -0 -r shellcheck -o all

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

# One skill eval in the nested niri: make nested-eval SCENARIO=compose-message SKILL=skills/niri-computer-use MODEL=sonnet
nested-eval:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run $(NESTED_FLAGS) --eval $(SCENARIO) --skill $(SKILL) --model $(MODEL)

host-capture:
	cargo run --locked -p harness -- host-capture $(OUTPUT)

sitting:
	cargo run --locked -p harness -- run --scale 1.5 --sitting
