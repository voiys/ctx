# Trace an API

Find the definition and follow re-exports from index.ts, lib.rs or __init__.py:

```sh
SRC=$(ctx path <dep>-<ver>-src)
rg -n 'export (async )?(function|const|class|type|interface) <X>\b|pub (async )?fn <x>\b|^def <x>\(' "$SRC"
```

Read types, defaults and real callers. Never answer from memory when pinned source is available.
Cite file:line @ SHA and verify the live callpath before applying the pattern.
