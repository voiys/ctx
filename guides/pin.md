# Pin an exact version

Read the real manifest/lockfile; map the installed version to an official tag or commit. Do not guess.

```sh
ctx add https://github.com/owner/repo/tree/<tag-or-sha> --label <dep>-<ver>-src --reason 'upstream source for locked version'
SRC=$(ctx path <dep>-<ver>-src)
git -C "$SRC" rev-parse HEAD
```

Read cached source with normal tools; never edit it or import from it. Different versions need different labels.
Keep the exact `ctx add` lines in a script or README for restoration. Re-running a cached label/URL is a no-op.
Use the URL shown by `ctx list`: substituting a SHA URL for a tag URL under the same label is a conflict.
Docs/papers restore with a fresh fetch, not the original snapshot.
