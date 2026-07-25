---
name: merge-herdr-upstream
description: Use when the user asks to merge or sync the latest stable Herdr upstream release into the z233 fork.
---

# Merge Herdr Upstream

Run a **gated merge** from upstream `origin` (`ogulcancelik/herdr`) into fork branch `z233`, whose remote is also `z233`. Meet each gate before continuing.

## 1. Establish the two ledgers

Read the repository `AGENTS.md` and `docs/FORK_FEATURES.md` completely. Then fetch both remotes and select the newest stable tag reachable from upstream master:

```sh
git remote get-url origin
git remote get-url z233
git fetch origin --tags --prune
git fetch z233 --prune
git rev-list --left-right --count z233...z233/master
release=$(git tag --merged origin/master --sort=-v:refname --list 'v*' |
  grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1)
```

The **upstream ledger** is release intent. The **fork ledger** is `docs/FORK_FEATURES.md` plus current fork tests.

Record the primary checkout's branch, porcelain status, and hashes of staged and unstaged binary diffs. Preserve state and ask if a remote is wrong, divergence is not `0 0`, or no stable tag exists. If `git merge-base --is-ancestor "$release" z233` succeeds, report the no-op and stop.

**Gate:** remote identities match, divergence is `0 0`, the release is unambiguous and not yet merged, and the primary-checkout ledger is recorded.

## 2. Isolate the merge

Create `merge/upstream-${release#v}` from `z233` under `../herdr-worktrees/merge-upstream-${release#v}`. Inspect rather than overwrite an existing branch or path.

```sh
git worktree add -b "merge/upstream-${release#v}" \
  "../herdr-worktrees/merge-upstream-${release#v}" z233
```

Work only in this worktree; the primary checkout is the recovery point.

**Gate:** the integration worktree is clean and its HEAD equals the recorded `z233` SHA.

## 3. Reconcile intent before editing

Inspect upstream-only commits, release notes, and the merge-base diff. Run the required roundtable covering upstream intent, fork invariants, and integration/test risk. Map every upstream-touched fork feature to intended behavior and focused tests; name missing characterization tests before editing.

**Gate:** every overlap with `docs/FORK_FEATURES.md` has an explicit preservation or intentional-change decision and a verification route.

## 4. Merge without committing

```sh
git merge --no-ff --no-commit "$release"
```

Inspect every protected surface even when Git reports a clean merge. For conflicts, load [`CONFLICTS.md`](CONFLICTS.md). Preserve the worktree and ask on product ambiguity.

**Gate:** `git ls-files -u` is empty, every conflict and protected overlap is accounted for, and `git diff --cached --check` passes.

## 5. Prove the result

Run focused fork regressions with `just test-one <filter>`, add black-box coverage where missing, then run `just check`. Review the staged diff against both ledgers and every applicable protocol, integration-version, vendored-patch, platform, and unreleased-doc rule in `AGENTS.md`.

On failure, keep the merge uncommitted. Reproduce suspected baseline failures from the pre-merge SHA, then fix the merge or present the exact comparison and obtain explicit acceptance of narrower verification.

**Gate:** every protected surface has passing evidence and `just check` passes, or the user explicitly accepts the documented narrower verification.

## 6. Request commit alignment

Present the audit, test evidence, and proposed message `chore: merge <release> into z233`. Approval gates are scoped: “finish,” deadlines, test exceptions, and push requests are not commit-message alignment.

**Gate:** the user responds to and accepts that exact proposal. Do not commit before this response. After alignment, load [`FINALIZE.md`](FINALIZE.md) and follow it through local integration and the separate push decision.

## Quick reference

Already merged: report the no-op. Failed or ambiguous gate: preserve the worktree and ask. Missing alignment: stop before commit. Dirty primary checkout: preserve its ledger throughout.

## Pressure traps

| Rationalization | Gate that still applies |
|---|---|
| “Upstream CI is green; take theirs.” | CI cannot prove fork invariants; reconcile both ledgers. |
| “Failures predate this merge.” | Evidence supports a user decision, not a waived gate. |
| “Finish also approves the commit.” | Obtain alignment on the exact message. |
