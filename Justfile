default: check

# format code
fmt:
    cargo fmt --all

# format check + clippy
check:
    cargo fmt --all --check
    cargo clippy --workspace -- -D warnings
    cargo clippy --workspace --features wsp/codegen -- -D warnings

# generate SKILL.md from CLI introspection
skill: (build-bin "codegen")
    cargo run --release -p wsp --features codegen -- generate > skills/wsp-manage/SKILL.md


# build a release binary, optionally with extra features
[private]
build-bin features="":
    {{ if features == "" { "cargo build --release -p wsp" } else { "cargo build --release -p wsp --features " + features } }}

# build release binary
build: check build-bin

# run all tests
test:
    cargo test --workspace -- --test-threads=1

# audit dependencies for known vulnerabilities
audit:
    cargo audit

# type-check against Windows and Linux to catch platform-specific errors
check-cross:
    rustup target add x86_64-pc-windows-msvc x86_64-unknown-linux-gnu 2>/dev/null || true
    cargo check --workspace --target x86_64-pc-windows-msvc
    cargo check --workspace --target x86_64-unknown-linux-gnu

# full local CI pipeline (superset of .github/workflows/ci.yml — also runs check-cross and SKILL.md freshness)
ci: check check-cross audit build test
    @echo "Checking SKILL.md freshness..."
    @cargo run --release -p wsp --features codegen -- generate | diff -q - skills/wsp-manage/SKILL.md || (echo "SKILL.md is stale. Run 'just skill' to regenerate." && exit 1)

# auto-fix formatting and lint where possible
fix:
    cargo fmt --all
    cargo clippy --workspace --fix --allow-dirty -- -D warnings

# preview unreleased changelog
changelog:
    git cliff --unreleased

# dry-run a release (patch, minor, or major)
release level:
    cargo release {{level}}

# execute a release (patch, minor, or major)
release-execute level:
    cargo release {{level}} --execute

# install git pre-commit hook
install-hooks:
    #!/usr/bin/env sh
    hooks_dir="$(git rev-parse --git-common-dir)/hooks"
    mkdir -p "$hooks_dir"
    cat > "$hooks_dir/pre-commit" <<'HOOK'
    #!/usr/bin/env sh
    just check
    HOOK
    chmod +x "$hooks_dir/pre-commit"
    echo "pre-commit hook installed to $hooks_dir/pre-commit"
