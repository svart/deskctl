# Tasks

Remove a task when it is done; the commit is the record. Spec:
`docs/SPEC.md`. Plan: `tasks/plan.md`.

Every task also: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
`cargo test` pass; one commit after the user agrees.

v1 is done: all spec success criteria met (session checks and the
us/ru layout check by the user on 2026-11-09).

## Next: a11y (docs/SPEC-a11y.md, plan in tasks/plan.md)

(a11y done: docs/SPEC-a11y.md criteria met on nested; see below for
the session.)

## Waiting for a user check

- `session a11y act` refusals on a real session app (auto mode stopped
  the agent from trying, 2026-11-09): the same checks as on nested
  (verified there), and the guard routing is unit-tested.

- Pointer on a second monitor (mapping across outputs is unit-tested only).
