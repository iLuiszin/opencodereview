### Added

- Repository subscriptions can name branches whose merged commits were already review-approved.
  Reviews and the pull request file limit then skip only the matching prefix already present on
  those branches, so promotion pull requests review only their new commits.
