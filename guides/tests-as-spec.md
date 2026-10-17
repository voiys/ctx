# Tests as specification

Use an exact pin, then inspect assertions:

```sh
SRC=$(ctx path <dep>-<ver>-src)
rg -n '<symbol>' "$SRC" -g '*test*' -g '*spec*' -g '**/tests/**'
```

Read inputs, errors, edge cases and defaults as the pinned contract. Follow helpers used by the test.
Write local tests for behavior that matters; do not mock away the behavior under test.
Cite file:line @ SHA and distinguish tested behavior from inference.
