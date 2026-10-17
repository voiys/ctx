# Idiomatic usage

Resolve an exact pin first (`ctx guides pin`). Search examples and tests before implementation details:

```sh
SRC=$(ctx path <dep>-<ver>-src)
rg -n '<Api>' "$SRC" -g '**/examples/**' -g '**/docs/**' -g '**/tests/**' -g '**/test/**'
rg -n '<Api>\(' "$SRC" -g '!**/dist/**' -g '!**/*.d.ts'
rg -n '<Api>' <repo>
```

Prefer repeated patterns from this pinned version, not remembered API shapes. The live repository convention wins.
Return the pattern with 1–3 file:line citations and the pin SHA.
