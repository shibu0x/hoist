BIN := hoist

.PHONY: help check fmt lint test build release install clean

help:
	@echo "check    fmt + lint + test (what CI runs)"
	@echo "fmt      format the code"
	@echo "lint     clippy, warnings are errors"
	@echo "test     run tests"
	@echo "build    debug build"
	@echo "release  optimised build"
	@echo "install  install $(BIN) into ~/.cargo/bin"
	@echo "clean    remove build artifacts"

check:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test

fmt:
	cargo fmt

lint:
	cargo clippy --all-targets -- -D warnings

test:
	cargo test

build:
	cargo build

release:
	cargo build --release

install:
	cargo install --path .

clean:
	cargo clean
