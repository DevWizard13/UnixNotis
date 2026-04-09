# Contributing to UnixNotis

Thanks for taking the time to contribute.

UnixNotis is a Wayland-first notification system with a D-Bus daemon, a control-center panel, toast popups, and supporting CLI and installer tooling. Contributions should improve the project without making it heavier, less predictable, or harder to maintain.

## Before contributing

- Read the [README](README.md) first
- Check the GitHub Wiki for configuration and styling details
- Keep changes focused
- Prefer fixing one problem well over mixing unrelated cleanup into the same pull request

## What we are looking for

Good contributions usually do one or more of these:

- Fix a bug or regression
- Improve reliability during long-running sessions
- Reduce unnecessary CPU, memory, or I/O work
- Improve maintainability without changing behavior
- Improve documentation, diagnostics, or test coverage
- Keep behavior aligned with the project's Wayland-first scope

## Scope and design expectations

Please keep contributions in scope for the project.

- Keep UnixNotis focused on being a notification system and related tooling
- Avoid feature creep that turns it into a general desktop shell or widget framework
- Do not add large new dependencies without a strong reason
- Do not add background work, polling, or timers unless they are clearly necessary and bounded
- Prefer event-driven behavior over periodic work when possible
- Keep memory use predictable and avoid unbounded queues, caches, or retries
- Avoid `unsafe` unless it has been discussed and there is a clear reason it cannot be avoided

## Performance expectations

Performance matters here.

- New code should avoid waste when idle
- Repeated work should be coalesced, cached, or bounded where appropriate
- Long-running processes should not slowly accumulate memory or CPU overhead
- Shell commands, subprocesses, watchers, and async tasks should have a clear lifecycle
- If a change touches hot paths, measure it instead of guessing

Good rules of thumb:

- Avoid busy loops
- Avoid waking the UI just to discover nothing changed
- Avoid doing the same parse, decode, or command twice when one result would do
- Avoid silent fallback behavior that hides broken or expensive paths

## Code style

- Keep files organized and responsibilities clear
- Prefer small focused modules over one oversized file
- Keep code readable and boring in the good way
- Add comments where they actually help future maintenance
- Avoid drive-by refactors unless they are necessary for the change
- Keep naming direct and consistent with the existing codebase

## Testing

For code changes, at a minimum, run:

```sh
cargo fmt --check
cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

## Pull requests

Please try to keep pull requests easy to review.

- Use a clear commit history
- Keep the PR focused on one problem or one closely related set of problems
- Explain the user-visible problem being solved
- Explain any behavior change, tradeoff, or limitation
- Include before/after measurements when claiming performance improvements
- Update docs when behavior or configuration changes

If docs live in the Wiki and are not updated in the PR, note what needs to be updated.

## Bug reports and fixes

If fixing a bug:

- Include the trigger or failure mode
- Include the root cause if it is known
- Prefer fixes that address the cause instead of hiding the symptom
- Add or update tests when practical

## Security and safety

- Treat all external input as untrusted
- Prefer bounded parsing, bounded reads, and bounded queues
- Avoid insecure defaults
- Avoid remote or shell-expanding behavior unless it is clearly intended and validated

If a change has security implications, call that out in the PR.

## AI-assisted contributions

AI-assisted contributions are allowed.

By submitting AI-assisted code, the contributor agrees to the following:

- The contributor accepts full responsibility for all code they submit
- The contributor has read, understood, and reviewed the generated code before submission
- The contributor has verified that the code is correct, in scope, and maintainable
- The contributor has checked for unnecessary complexity, regressions, and waste
- The contributor is responsible for licensing, attribution, and policy compliance for submitted content

AI output is not an excuse for low-quality, unreviewed, or unexplained changes.

## Ways to support the project

> And if the project is useful, but there is no time to contribute code, that is fine too. There are other easy ways to support the project and show appreciation:
> - Star the project
> - Tweet or post about it
> - Mention UnixNotis in another project's README
> - Mention the project at local meetups
> - Tell friends or coworkers who might find it useful
> - Report bugs clearly and with enough detail to reproduce them

## Questions

If something is unclear, ask before making a large change.

Small, focused, well-tested contributions are much more likely to land cleanly than large rewrites.
