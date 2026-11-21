### Merge-queue ejections record their failing tests again

The CI watcher now reads real `gh run view --log-failed` output, so it finds failing tests after a merge-queue ejection. It strips each line's job, step and timestamp prefix and gh's colour codes, then recognises both libtest `test <name> ... FAILED` lines and nextest `FAIL [..]` lines. Before this, the watcher never matched a real line, so the class-audit gate never fired after an ejection.
