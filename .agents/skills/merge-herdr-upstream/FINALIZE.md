# Finalize the Herdr Upstream Merge

Load this only after the user accepts the exact merge commit message proposed with the audit and test evidence.

## 1. Commit and verify

Commit the staged merge with the approved message. Verify it has exactly two parents: parent one is the recorded fork SHA and parent two is `"$release^{}"`. The integration worktree must then be clean.

## 2. Integrate locally

In the primary checkout, verify the recorded status and staged/unstaged diff hashes, then fast-forward local `z233` to the integration branch. Recheck the ledger byte-for-byte. If local changes block the fast-forward, retain the worktree and branch and report the blocker; the recovery artifact is more important than cleanup.

## 3. Clean only owned state

After successful local integration, remove the integration worktree and delete its branch with `git branch -d`. Report the merge SHA, parent SHAs, protected-surface audit, and test evidence.

## 4. Gate the push separately

Request explicit approval to push. Commit-message alignment, “finish,” deadlines, and prior general instructions do not approve remote mutation. Only after the user approves:

```sh
git push z233 z233:master
git ls-remote z233 refs/heads/master
```

**Completion criterion:** local `z233` contains the merge, unrelated changes remain byte-for-byte preserved, temporary state is safely removed, and any pushed SHA exactly matches local `z233`.

| Pressure trap | Required behavior |
|---|---|
| “Push direct; repair local later.” | Integrate locally and preserve the ledger first. |
| “The deadline implies push.” | Push has its own explicit approval gate. |
| “Scratch state is disposable.” | Clean only verified artifacts created by this run. |
