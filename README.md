# Voxam

_A Specification-Accurate Z-Machine Implementation_

Voxam is a Z-Machine interpreter written in Rust, targeting story file
versions 1 through 8. That covers the Infocom-era formats (v1-v6) and the
later extensions that Inform emits (v7 and v8).

## Building

```sh
cargo build
cargo build --release

cargo run
cargo run --release
```

## Development

Commit messages follow the
[Conventional Commits](https://www.conventionalcommits.org/) specification,
enforced locally by [cocogitto](https://github.com/cocogitto/cocogitto).
The hook definitions live in `cog.toml`, but Git hooks themselves never
travel with a clone, so activating enforcement is a one-time step per
machine:

```sh
cargo install cocogitto
cog install-hook --all
```

After that, any commit whose message does not parse as a conventional
commit is rejected at commit time. To check a message without
committing:

```sh
cog verify "feat: add object table parsing"
```
