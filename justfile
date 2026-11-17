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

# Squash a local branch onto main as one commit; validates arguments before any git command.
land branch message="": (_land-check branch message)
    git diff --quiet && git diff --cached --quiet
    BRANCH={{quote(branch)}} sh -c 'git switch "$BRANCH"'
    just ci
    git switch main
    BRANCH={{quote(branch)}} sh -c 'git merge --squash "$BRANCH"'
    MESSAGE={{quote(message)}} sh -c 'if [ -n "$MESSAGE" ]; then git commit -F "$MESSAGE"; else git commit; fi'
    cargo xtask commits HEAD~1..HEAD
    BRANCH={{quote(branch)}} sh -c 'git branch -D "$BRANCH"'

# Refuse anything that is not a plain branch name or an existing message file.
[private]
_land-check branch message="":
    BRANCH={{quote(branch)}} sh -c 'case "$BRANCH" in ""|-*|*[!A-Za-z0-9._/-]*) echo "land: invalid branch name: $BRANCH" >&2; exit 1;; esac; git check-ref-format --branch "$BRANCH" >/dev/null'
    MESSAGE={{quote(message)}} sh -c 'if [ -n "$MESSAGE" ] && [ ! -f "$MESSAGE" ]; then echo "land: message is not a file: $MESSAGE" >&2; exit 1; fi'
