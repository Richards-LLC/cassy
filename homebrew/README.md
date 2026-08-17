# Homebrew Tap for CAS

This directory contains the Homebrew formula for CAS.

## Setup Instructions

### 1. Add the Formula

Copy `cas.rb` to the tap repository:

```
homebrew-cas/
└── Formula/
    └── cas.rb
```

### 2. Update SHA256 Hashes

After a release, update the formula with the correct hashes:

```bash
./update-formula.sh 0.2.0
```

Then commit and push to the tap repository.

## User Installation

Install the checked-out formula directly:

```bash
brew install --formula ./homebrew/cas.rb
```

## Updating the Formula

When releasing a new version:

1. Create and push the git tag (triggers release workflow)
2. Wait for GitHub Actions to build and publish the release
3. Run `./update-formula.sh <version>` to update hashes
4. Commit and push the updated formula to the tap repository

## Alternative: Automated Updates

For automated formula updates, consider adding a GitHub Action to the tap repository that:

1. Watches for new releases in the main CAS repo
2. Downloads the new binaries and computes SHA256
3. Updates the formula and creates a PR
