.PHONY: check lint coverage nested host-capture

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

nested:
	cargo build --locked --manifest-path probes/vpointer/Cargo.toml
	cargo run --locked -p harness -- run --scale $(SCALE)

host-capture:
	cargo run --locked -p harness -- host-capture $(OUTPUT)
