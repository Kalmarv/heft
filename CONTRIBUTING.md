# Contributing

## Releasing

1. Raise the version in the workspace and in the crates' dependencies on each other (a patch, 0.x.Y, for fixes and data
   or manifest additions; a minor, 0.X.0, for new API or any breaking change before 1.0; then `cargo check`, so
   `Cargo.lock` follows) and put the change's CHANGELOG lines under a `## <version> (<date>)` heading, in one commit.
2. Push it to main. Once every check has passed, release-plz tags the commit `v<version>` and creates the GitHub Release
   with that CHANGELOG section as its notes. Then dist (`.github/workflows/release.yml`) builds the binaries and
   attaches them to the Release, with their checksums.

A commit that raises no version only runs CI. Never release by hand. If a run fails after the tag was made, start
`release.yml` by hand with that tag.
