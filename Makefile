BINARY_NAME := tmx
INSTALL_DIR := $(HOME)/.local/bin

.PHONY: build install uninstall clean test fmt lint check

build:
	cargo build --release

install: build
	mkdir -p $(INSTALL_DIR)
	cp target/release/$(BINARY_NAME) $(INSTALL_DIR)/$(BINARY_NAME)

uninstall:
	rm -f $(INSTALL_DIR)/$(BINARY_NAME)

clean:
	cargo clean

test:
	cargo test

fmt:
	cargo fmt

lint:
	cargo clippy --all-targets -- -D warnings

check: fmt lint test
