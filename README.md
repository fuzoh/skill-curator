# skill-curator

A configurable tool for downloading, adapting, and maintaining AI coding-agent
skills on the user's machine.

**Status: early development.** `materialize` and `install` are implemented;
`status`, `check`, and `update` are not implemented yet.

## Building

Requires Rust (the release pinned in `rust-toolchain.toml`) and, at runtime,
system `git`.

```sh
cargo build
cargo run -- --help
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the quality checks.

## Goals

- Select skills from multiple upstream repositories and pin their versions.
- Keep skills unchanged or adapt them to a user's tools and agent environments.
- Review upstream updates without losing local customizations.
- Track sources, adaptation history, and installation choices.

The tool will not bundle or redistribute third-party skills. Users fetch them
locally from their original sources; upstream licenses still apply.

## Planning

- [Goals and candidate features](docs/goals.md)
- [Design ideas and open questions](docs/design-notes.md)

## License

[MIT](LICENSE). Covers this project's own code and documentation, not downloaded
third-party content.
