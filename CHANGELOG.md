# Changelog

<!-- esc:version 0.9.0-dev -->
<!-- Rule 0e: the marker above and the four other version locations must all agree.
     Enforced by scripts/version-consistency.sh; update them together. -->

All notable changes to this project are documented here, in
[Common Changelog](https://common-changelog.org) form.

**Do not edit the version sections below by hand.** They are assembled by
`cargo xtask notes --version X.Y.Z` from the fragments in `.changes/`, one per
pull request. Editing here and not there means the next release silently drops
the edit.

Three properties of this format were chosen against problems this project has
actually had, rather than as convention (doctrine 05 §4.1):

- **Every entry references a PR**, so a one-line claim has a path back to the
  diff. This project has been burned by claims with no path back to evidence.
- **There is no `Unreleased` section.** It is where entries get written before
  the change settles, and it is a standing merge conflict.
- **Breaking changes are prefixed `**Breaking:**` inline**, not filed in a
  category a reader can skip.

The engineering journal — dated, free-form, and keeping its retractions — moved
to `docs/changelog-journal.md` and does not ship.

<!-- esc:notes-begin -->
<!-- esc:notes-end -->
