.PHONY: help version-bump release build test clean clippy fmt fmt-check lint install-hooks run

# Auto-generate version from today's date with auto-incrementing patch.
# Format: YYYYMMDD.0.X where X increments for multiple releases per day.
define get_next_version
$(shell \
	TODAY=$$(date +%Y%m%d); \
	LATEST=$$(git tag -l "v$$TODAY.*" 2>/dev/null | sort -V | tail -1); \
	if [ -z "$$LATEST" ]; then \
		echo "$$TODAY.0.0"; \
	else \
		PATCH=$$(echo "$$LATEST" | sed 's/.*\.0\.\([0-9]*\)/\1/'); \
		echo "$$TODAY.0.$$((PATCH + 1))"; \
	fi \
)
endef

VERSION := $(get_next_version)

help:
	@echo "albo Makefile"
	@echo ""
	@echo "  make release        - lint, tag vDATE, push; GitHub Actions builds"
	@echo "                        the Linux binary and cuts the release"
	@echo "  make run            - run the server against directory.toml"
	@echo "  make build/test/lint/clippy/fmt/clean"
	@echo ""
	@echo "Next version will be: $(VERSION)"

# Tag and push. Unlike a crate, the artifact is built by CI on the tag
# (see .github/workflows/release.yml), so release just needs a clean tree,
# a passing lint, and a pushed tag. Lint runs first so a broken build never
# gets tagged.
release: lint
	@echo "Releasing v$(VERSION)..."
	@git diff --quiet || { echo "working tree dirty; commit first"; exit 1; }
	@git tag -a v$(VERSION) -m "Release v$(VERSION)"
	@git push origin main
	@git push origin v$(VERSION)
	@echo ""
	@echo "Tagged and pushed v$(VERSION). GitHub Actions is building the"
	@echo "release binary now. Once it lands, update pond-nix to the new"
	@echo "url + sha256 (nix-prefetch-url the release asset)."

run:
	cargo run -- serve

build:
	cargo build --release

test:
	cargo test

clippy:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt -- --check

clean:
	cargo clean

# Everything CI runs, locally before pushing.
lint: fmt-check
	cargo clippy --all-targets -- -D warnings
	cargo test

# Pre-push hook that runs `make lint`, catching CI failures locally.
install-hooks:
	@mkdir -p .git/hooks
	@printf '#!/usr/bin/env bash\nset -e\nexec make lint\n' > .git/hooks/pre-push
	@chmod +x .git/hooks/pre-push
	@echo "Installed pre-push hook -> make lint"
