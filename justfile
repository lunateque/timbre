set shell := ["sh", "-eu", "-c"]
set windows-shell := ["sh", "-eu", "-c"]

# The whole quality gate, in the order of the design; stops at the first failure.
ci:
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    cargo xtask check
    cargo deny --locked check
    cargo machete
    typos
    cargo test --workspace --all-features --locked
    cargo xtask removability
    RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --locked

# Apply formatting and the machine-applicable clippy fixes.
fix:
    cargo fmt --all
    cargo clippy --fix --allow-dirty --allow-staged --workspace --all-targets --all-features

# Enable the hooks and install the pinned tools.
setup:
    git config core.hooksPath .githooks
    cargo install --locked cargo-deny@0.20.2 cargo-machete@0.10.1 typos-cli@1.52.2 committed@1.1.12

# Squash a local branch onto main as one polished commit; the message comes from the
# editor, or from `message` (a file path) when landing without a terminal.
land branch message="":
    git diff --quiet && git diff --cached --quiet
    git switch {{branch}}
    just ci
    git switch main
    git merge --squash {{branch}}
    if [ -n "{{message}}" ]; then git commit -F "{{message}}"; else git commit; fi
    cargo xtask commits HEAD~1..HEAD
    git branch -D {{branch}}
