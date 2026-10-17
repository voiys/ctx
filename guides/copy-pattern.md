# Copy a pattern

Use an exact source pin (`ctx guides pin`). Find how upstream composes the abstraction:

```sh
SRC=$(ctx path <dep>-<ver>-src)
rg -l '<Abstraction>' "$SRC" -g '**/src/**'
```

Read a full call site, then mirror its error handling, resource scoping, and configuration.
Reuse existing repository helpers instead of hand-rolling another layer. Cite file:line @ SHA.
