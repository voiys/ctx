# Upgrade impact

Pin old and new versions under separate labels (`ctx guides pin`), then compare relevant subpaths:

```sh
OLD=$(ctx path <old-label>)
NEW=$(ctx path <new-label>)
git diff --no-index --stat "$OLD/src" "$NEW/src"
git diff --no-index "$OLD/src" "$NEW/src"
rg -n -i 'breaking|removed|renamed|deprecat' "$NEW" -g '*CHANGELOG*' -g '*MIGRATION*' -g '**/docs/**'
rg -n -w '<name1>|<name2>' <repo>
```

If there is no src directory, compare existing public-entry subpaths instead. Never diff whole pins: they contain .git.
Diff exit status 1 means differences, not failure. Map every affected use, including wrappers, re-exports, types,
mocks, fixtures and snapshots; exclude vendored code as appropriate.
Done means build and full tests pass, with zero remaining uses of removed symbols. Cite both pin SHAs.
