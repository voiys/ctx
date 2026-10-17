# Why: history (optional)

Cached clones are shallow. Only when history is necessary and network access is permitted:

```sh
SRC=$(ctx path <dep>-<ver>-src)
git -C "$SRC" fetch --deepen=500
git -C "$SRC" log -S '<symbol>' --oneline
git -C "$SRC" log -L :<function>:<file>
```

Use --unshallow instead if complete history is necessary. Deepening changes cached history, never pinned HEAD.
Never edit cached files. History explains intent, not current behavior; verify the live code and cite file:line @ SHA.
