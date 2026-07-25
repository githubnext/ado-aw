# Local Development

## Prerequisites

- Rust 1.94.0 or later
- Git

## Build and test

From the repository root:

```bash
cargo build
cargo test
cargo clippy --all-targets --all-features
```

## Test compilation

Compile a workflow from source, then verify its generated pipeline:

```bash
cargo run -- compile --force path/to/agent.md
cargo run -- check path/to/agent.lock.yml
```

## Documentation site

The documentation site lives in `site/`:

```bash
cd site
npm ci
npm run build
```
