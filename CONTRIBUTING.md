# Contributing

## Releasing

A release is a commit that changes the version of a package that ships a binary (`heft`, `heft-mft`). Once every CI job has
passed on main, CI's release job (`.github/release.py`) builds the binaries, tags the commit (`v<version>`, or
`<package>-v<version>` where several packages ship) and creates the GitHub Release with the commits since the previous release as its notes and
`<bin>-<version>-<target>.zip` / `.tar.gz` and `SHA256SUMS` attached. A commit that changes no version only runs CI.

1. Bump the package's `version` when a change is worth releasing: a patch (0.x.Y) for fixes, a minor (0.X.0) for
   new features or changed behaviour. Run `cargo check` so `Cargo.lock` follows.
2. Push to main. Never tag or create a Release yourself.
