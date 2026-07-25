# Reconciling Herdr Merge Conflicts

Load this only when `git merge --no-ff --no-commit <release>` reports conflicts.

## Evidence loop

For every path from `git diff --name-only --diff-filter=U`:

1. Inspect the base, fork, and upstream stages with `git show :1:<path>`, `:2:<path>`, and `:3:<path>`. In this merge, stage 2 is the `z233` fork and stage 3 is the upstream release.
2. Trace both histories with `git log -p -- <path>` from the recorded fork SHA and release. Follow renames when either side moved the code.
3. Map each conflicting hunk to upstream intent, a fork invariant from `docs/FORK_FEATURES.md`, and a test. Generated files map to their source and regeneration command instead of a hand-edited resolution.
4. Edit a synthesis that preserves both intents unless the protected-surface audit records an intentional replacement. Stage the path only after reviewing its complete diff.
5. Run the narrowest relevant `just test-one <filter>` immediately. Record the decision and evidence before moving to the next path.

**Completion criterion:** every conflicted path has a recorded intent decision, no unmerged index stages remain, generated outputs match their sources, and all mapped focused tests pass.

## Example: sidebar conflict

Suppose upstream restructures grouped workspace rows while the fork configures entry gaps. Read the commits that introduced both changes, locate the current row-layout tests, and resolve the functions so upstream grouping still applies while fork gap tokens remain at the documented boundary. Add or adapt a black-box layout test that exercises both grouping and configured gaps, then run its `just test-one` filter. Choosing the whole fork or upstream file is not reconciliation because either choice erases one ledger.

## Conflict-specific traps

| Shortcut | Evidence-driven replacement |
|---|---|
| Whole-file `ours` or `theirs` | Resolve hunk-by-hunk; use a whole side only when the audit proves the other side has no live intent. |
| “Start from ours” for fork docs | Compare upstream additions first, then produce one current document. |
| Hand-combine schemas or generated references | Resolve the source and run the repository generator/check. |
| Passing compiler as proof | Run the mapped behavior tests and the full proof gate. |
| Resolve all files, test once | Test each protected surface while its reasoning is fresh. |
