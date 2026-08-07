# CAS CLI Makefile
# Local development and testing utilities
#
# ## Testing Strategy
#
# This project uses a multi-layered testing approach:
# - Unit tests: `make test` or `make unit`
# - Integration tests: `make integration`
# - Coverage reports: `make coverage` or `make coverage-html`
#
# ## UI Testing Policy
#
# The `src/ui/` module (TUI components) is EXCLUDED from automated testing.
# Reasons:
# 1. TUI code relies heavily on terminal state and user interaction
# 2. ratatui widgets require mocked terminal backends for testing
# 3. Visual correctness is better verified through manual testing
# 4. ROI on UI unit tests is low for this CLI tool
#
# UI code is excluded from coverage metrics via `--exclude-files "src/ui/*"`.
# Manual testing checklist for UI changes:
# - [ ] TUI renders correctly in different terminal sizes
# - [ ] Keyboard navigation works (j/k, arrow keys, Enter, Esc)
# - [ ] Color scheme is readable in light/dark terminals
# - [ ] Status line updates correctly
# - [ ] No visual artifacts or flickering

.PHONY: test test-full test-clean-env coverage coverage-html clean build build-full release release-fast test-release-panic test-release-panic-release test-release-panic-release-fast check check-full lint dev-build dev-test timings benchmark-build check-build-regression

CARGO ?= cargo

# Run all tests
test:
	$(CARGO) test -p cas

# Run full test matrix
test-full:
	$(CARGO) test -p cas --all-features

# Run tests with output
test-verbose:
	$(CARGO) test -p cas -- --nocapture

# Reproduce the clean-CI environment shape locally (cas-43ce / GH #136).
#
# Every factory worker shell exports CAS_* identity variables. A test that
# accidentally reads one passes here and fails only on a clean CI runner —
# that is exactly how the GH #136 tests shipped red, and this command
# surfaced it in seconds. Run it before pushing when your diff touches
# agent resolution, coordination, messaging or anything else that reads the
# environment.
#
# The variable list is enumerated from the live environment rather than
# hardcoded, because a hardcoded one drifts: the list in the worker guide
# had gone stale in both directions (missing CAS_CLOUD_TOKEN among others,
# stripping two variables that no longer exist).
#
# NOTE: this MUST stay an explicit rule — the `test-%` pattern rule below
# would otherwise match `test-clean-env` and run `cargo test -p cas clean-env`.
#
# Scope it, as always. Default is one binary; override for another:
#   make test-clean-env
#   make test-clean-env CLEAN_ENV_ARGS='--lib cloud::config'
CLEAN_ENV_ARGS ?= --test factory_mcp_ops_test
test-clean-env:
	@stripped=$$(env | sed -n 's/^\(CAS_[A-Za-z0-9_]*\)=.*/\1/p' | sort); \
	if [ -n "$$stripped" ]; then \
		echo "Stripping $$(echo "$$stripped" | wc -l) CAS_* variable(s): $$(echo $$stripped)"; \
	else \
		echo "No CAS_* variables set — already in the clean-CI shape."; \
	fi; \
	unset_args=$$(echo "$$stripped" | sed 's/^/-u /'); \
	env $$unset_args $(CARGO) test -p cas $(CLEAN_ENV_ARGS)

# Run specific test module
test-%:
	$(CARGO) test -p cas $* -- --nocapture

# Generate coverage report (requires cargo-tarpaulin)
# Install: cargo install cargo-tarpaulin
coverage:
	$(CARGO) tarpaulin --all-features --workspace --timeout 300 \
		--ignore-tests \
		--exclude-files "src/ui/*" \
		--out Stdout

# Generate HTML coverage report
coverage-html:
	$(CARGO) tarpaulin --all-features --workspace --timeout 300 \
		--ignore-tests \
		--exclude-files "src/ui/*" \
		--out Html \
		--output-dir target/coverage
	@echo "Coverage report: target/coverage/tarpaulin-report.html"

# Generate coverage with specific modules
coverage-hooks:
	$(CARGO) tarpaulin --all-features --timeout 300 \
		--ignore-tests \
		--files "src/hooks/*" \
		--out Stdout

coverage-search:
	$(CARGO) tarpaulin --all-features --timeout 300 \
		--ignore-tests \
		--files "src/search/*" \
		--out Stdout

coverage-tracing:
	$(CARGO) tarpaulin --all-features --timeout 300 \
		--ignore-tests \
		--files "src/tracing/*" \
		--out Stdout

coverage-extraction:
	$(CARGO) tarpaulin --all-features --timeout 300 \
		--ignore-tests \
		--files "src/extraction/*" \
		--out Stdout

coverage-mcp:
	$(CARGO) tarpaulin --all-features --timeout 300 \
		--ignore-tests \
		--files "src/mcp/*" \
		--out Stdout

# Build debug
build:
	$(CARGO) build -p cas

# Build debug with all features
build-full:
	$(CARGO) build -p cas --all-features

# Build without the optional MCP proxy
dev-build:
	$(CARGO) build -p cas --no-default-features

# Build release
release:
	$(CARGO) build -p cas --release --all-features

# Build faster release profile for CI/pre-release checks
release-fast:
	$(CARGO) build -p cas --profile release-fast

# Run A2/A3/B3 panic-isolation tests under release profiles (EPIC cas-c351).
# Verifies the MCP panic catcher survives release-mode LTO + inlining and
# that the release-fast profile has not re-introduced panic=abort.
#
# Split into one target per profile so CI can run the two profiles as parallel
# jobs (they are two independent full LTO builds; in series they were a 30-min
# step). `test-release-panic` still runs both, so the local command documented
# in CLAUDE.md is unchanged.
test-release-panic: test-release-panic-release test-release-panic-release-fast

test-release-panic-release:
	$(CARGO) test -p cas --release --lib panic_regression
	$(CARGO) test -p cas --release --test search_utf8_regression_test

test-release-panic-release-fast:
	$(CARGO) test -p cas --profile release-fast --lib panic_regression
	$(CARGO) test -p cas --profile release-fast --test search_utf8_regression_test

# Type check
check:
	$(CARGO) check -p cas

# Type check full matrix
check-full:
	$(CARGO) check -p cas --all-features

# Lint with clippy
lint:
	$(CARGO) clippy -p cas --all-features -- -D warnings

# Format code
fmt:
	$(CARGO) fmt

# Clean build artifacts
clean:
	$(CARGO) clean

# Install development tools
install-tools:
	$(CARGO) install cargo-tarpaulin
	$(CARGO) install cargo-nextest

# Run tests with nextest (faster parallel execution)
test-nextest:
	$(CARGO) nextest run -p cas --all-features

# Watch for changes and run tests
watch:
	$(CARGO) watch -x "test -p cas"

# Integration tests only
integration:
	$(CARGO) test -p cas --all-features --test "*" -- --test-threads=1

# Unit tests only (lib tests)
unit:
	$(CARGO) test -p cas --lib

# Run library tests without the optional MCP proxy
dev-test:
	$(CARGO) test -p cas --lib --no-default-features

# Generate cargo timing report
timings:
	$(CARGO) build -p cas --timings

# Benchmark build times and write history/build-benchmark-latest.{md,env}
benchmark-build:
	../scripts/benchmark-build.sh

# Check build-time regression against history/build-benchmark-baseline.env
check-build-regression:
	../scripts/check-build-regression.sh
