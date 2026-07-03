# List available recipes
default:
    @just --list

# ---------- Build ----------

# Build library + binary
build:
    cargo build

# Build binary only
build-bin:
    cargo build --bin http-credential-broker

# Build the container image locally
container-build:
    docker build -t http-credential-broker:local .

# ---------- Lint ----------

# Run clippy with warnings as failures
clippy:
    cargo clippy --all-targets -- -D warnings

# Check formatting
fmt-check:
    cargo fmt --all -- --check

# Apply formatting
fmt:
    cargo fmt --all

# ---------- Test ----------

# Run all tests
test:
    cargo test

# Run one test filter
test-filter filter:
    cargo test {{ filter }}

# ---------- Docs ----------

# Build rustdoc with warnings as errors
doc-check:
    RUSTDOCFLAGS="-Dwarnings" cargo doc --no-deps

# ---------- Release ----------

# Check the cargo-dist release plan
dist-plan:
    dist plan

# Regenerate cargo-dist GitHub workflow
dist-generate:
    dist generate --mode ci

# ---------- CI ----------

# Run the full CI pipeline locally
ci: fmt-check clippy test doc-check
