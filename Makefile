BINARY_NAME=tk

.PHONY: fmt lint build test clean install completions

fmt:
	cargo fmt --all

lint:
	cargo clippy --all-targets -- -D warnings

build:
	cargo build --release
	cp target/release/$(BINARY_NAME) $(BINARY_NAME)

test:
	cargo test --all-targets

clean:
	cargo clean
	rm -f $(BINARY_NAME)

install:
	cargo install --path .

# Print fish completions to stdout (requires usage CLI: https://usage.jdx.dev)
completions:
	./target/debug/tk __usage_spec__ | usage generate completion fish tk --usage-cmd "tk __usage_spec__"
