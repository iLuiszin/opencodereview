### Added

- Repository subscriptions can name branches whose merged commits were already review-approved.
  Reviews and the pull request file limit then skip only the matching prefix already present on
  those branches, so promotion pull requests review only their new commits.
- Pull requests skipped because they exceed the file limit or contain only excluded files now get a
  machine-readable skipped result comment on GitHub, updated for each new head.
