# Contributing to Aurelia

Thanks for your interest in Aurelia! This project and everyone participating in it is
governed by our [Code of Conduct](CODE_OF_CONDUCT.md) — by taking part, you agree to uphold
it. Contributions of every kind are welcome:

- Reporting a bug
- Discussing the current state of the code
- Submitting a fix
- Proposing or building a new feature
- Improving documentation

Aurelia is a pure-Rust command-line Steam client. If you're new to the codebase, start
with the [README](README.md) for the big picture and [USAGE.md](USAGE.md) for what the
CLI actually does.

## Getting set up

You'll need a [Rust toolchain](https://rustup.rs/) (edition 2024). On Linux, install the
system dependencies listed under [Prerequisites](README.md#prerequisites) first.

```bash
git clone https://github.com/Drackrath/Aurelia.git
cd Aurelia
cargo build
cargo test
```

## Changes happen through pull requests

Pull requests are the way to propose changes. The typical flow:

1. Check the [issues](https://github.com/Drackrath/Aurelia/issues) to see if someone is
   already working on it. For anything non-trivial, open an issue first so we can agree on
   the approach before you invest time.
2. Fork the repo and create a branch off `main`.
3. Make your change. Keep the PR focused — one logical change per PR is much easier to
   review than a large mixed one.
4. Make sure the checks below pass.
5. Open the PR with a clear description of *what* changed and *why*. Link any related
   issue. Draft PRs are great for work-in-progress or to ask for early feedback.
6. A maintainer will review when they have time. If it's gone quiet for a few days, feel
   free to ping.

## Before you submit

Run these locally and make sure they're clean:

```bash
cargo fmt --all          # format (CI expects `cargo fmt --all -- --check` to pass)
cargo clippy --all-targets   # lint — don't add new warnings
cargo test               # run the test suite
```

A few expectations:

- **Formatting:** code is formatted with `rustfmt`. Run `cargo fmt` before committing.
- **Lints:** don't introduce new `clippy` warnings. The codebase has some pre-existing
  ones; you don't have to fix those, but don't add more.
- **Tests:** add or update tests for behavior you change. Pure logic (parsers, VDF/ACF
  handling, state derivation, etc.) should have unit tests; look at the `#[cfg(test)]`
  modules in `src/` for the existing style. Network- or Steam-dependent code can't be
  unit-tested directly, so factor the testable logic out into pure functions.

## Coding style

- **Match the surrounding code.** Follow the conventions already in the file or module
  you're editing — naming, error handling, comment density, and structure.
- **Errors:** use `anyhow::Result` with `.context()` / `.with_context()` to add helpful
  messages, as the rest of the codebase does. Reserve `bail!` for genuine failures.
- **Comments explain *why*, not *what*.** The code says what it does; a comment should
  explain a non-obvious reason, a protocol quirk, or a Steam-specific gotcha.
- **Keep stdout clean.** Diagnostics go to stderr (via `tracing`) so that `--json` output
  on stdout stays machine-readable. Every user-facing command should support `--json`.
- **Async:** the client is built on `tokio`. Prefer the existing async patterns over
  blocking calls inside async contexts.

## Reporting bugs

We track bugs through [GitHub issues](https://github.com/Drackrath/Aurelia/issues).
[Open a new issue](https://github.com/Drackrath/Aurelia/issues/new) and include:

- A short summary and any relevant background
- Steps to reproduce — be specific
- What you expected to happen
- What actually happened (full error output helps; re-run with `--json` or with
  `RUST_LOG=debug` for more detail)
- Your OS and how you installed/built Aurelia

> ⚠️ Please **redact credentials, refresh tokens, and the contents of `session.json`**
> from any logs or screenshots you attach.

## License

By contributing, you agree that your contributions will be licensed under the project's
[GPL-3.0 License](LICENSE), the same license that covers Aurelia.
