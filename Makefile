.PHONY: check lint coverage inspect inspect-check nested nested-control nested-actions nested-input sitting host-capture

SCALE ?= 1

check:
	cargo fmt --check
	cargo clippy --all-targets --locked -- -D warnings
	RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked
	cargo test --all-targets --locked
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
	cargo build --locked --manifest-path probes/noctalia-socket/Cargo.toml
	cargo run --locked -p harness -- run --scale $(SCALE) $(if $(NOCTALIA),--noctalia)

nested-control:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run --control

nested-actions:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run --actions

nested-input:
	cargo build --locked -p niri-computer-use
	cargo run --locked -p harness -- run --scale $(SCALE) --input

host-capture:
	cargo run --locked -p harness -- host-capture $(OUTPUT)

sitting:
	cargo run --locked -p harness -- run --scale 1.5 --sitting
