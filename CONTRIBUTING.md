# Contributing

## Commit messages

```
<type>(<scope>): <summary>

<body: what changed and why, wrapped at 72 columns>

<footer: Closes #12, BREAKING CHANGE: …, Co-Authored-By: …>
```

- **type**: `feat` (new feature), `fix` (bug fix), `perf`, `refactor` (no behaviour change),
  `docs`, `test`, `build` (Cargo, scripts, packaging), `ci`, `chore` (anything else).
- **scope** (optional): the area touched, usually a module: `terminal`, `bookmarks`,
  `archive`, `remote`, `preview`, `settings`, `i18n`, `macos`, …
- **summary**: imperative, lower case, no final period, at most 72 characters
  (`add`, not `added` / `adds`).
- **body** (optional): explain *why* and any user-visible behaviour; skip it when the
  summary says it all.
- A breaking change (settings file, command line, shortcuts) adds `!` after the type
  (`feat(settings)!: …`) and a `BREAKING CHANGE:` footer.
- One logical change per commit; keep translations (`i18n_data.rs`) with the feature
  that needs them.

Examples:

```
feat(bookmarks): cd the terminal to a bookmark when it is focused
fix(archive): keep the cursor on the file after extracting
build(macos): build a universal DMG
```

## Releases

Tags are `v<version>` from `Cargo.toml` (e.g. `v0.1.0`); the GitHub release carries
the DMG from `make dmg-universal`.
