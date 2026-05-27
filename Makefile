BINARY_NAME := tmx
INSTALL_DIR := $(HOME)/.local/bin

.PHONY: build install uninstall clean test fmt lint check release

build:
	cargo build --release

install: build
	mkdir -p $(INSTALL_DIR)
	# install(1) atomically unlinks+rewrites — `cp` fails with "Text file
	# busy" on Linux when the target binary is already running (another
	# tmx session open, dashboard in another terminal, etc).
	install -m 755 target/release/$(BINARY_NAME) $(INSTALL_DIR)/$(BINARY_NAME)
	@if [ "$$(uname)" = "Darwin" ]; then \
		codesign --force --sign - "$(INSTALL_DIR)/$(BINARY_NAME)"; \
	fi

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

# Cut a release: bump Cargo.toml version, sync Cargo.lock, commit, tag.
# Usage: make release VERSION=0.1.3
# The tag push triggers .github/workflows/release.yml (cargo-dist).
release:
	@if [ -z "$(VERSION)" ]; then \
		echo "Usage: make release VERSION=X.Y.Z"; exit 1; \
	fi
	@echo "$(VERSION)" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([.-].+)?$$' || { \
		echo "Error: VERSION must look like X.Y.Z (got '$(VERSION)')"; exit 1; \
	}
	@if [ -n "$$(git status --porcelain)" ]; then \
		echo "Error: working tree is dirty. Commit or stash first."; exit 1; \
	fi
	@if git rev-parse "v$(VERSION)" >/dev/null 2>&1; then \
		echo "Error: tag v$(VERSION) already exists."; exit 1; \
	fi
	@echo "==> Bumping Cargo.toml to $(VERSION)"
	@sed -i.bak -E 's/^version = "[^"]+"/version = "$(VERSION)"/' Cargo.toml
	@rm -f Cargo.toml.bak
	@echo "==> Syncing Cargo.lock"
	@cargo check --quiet
	@echo "==> Committing and tagging"
	@git add Cargo.toml Cargo.lock
	@git commit -m "Release v$(VERSION)"
	@git tag -a "v$(VERSION)" -m "v$(VERSION)"
	@echo ""
	@echo "Tagged v$(VERSION). Push to trigger the release workflow:"
	@echo "  git push --follow-tags origin master"
