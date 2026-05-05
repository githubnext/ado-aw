# @ado-aw/scripts

Bundled TypeScript scripts shipped in `scripts.zip` alongside the ado-aw release.

## Bundles

- `gate.js` — trigger filter gate evaluator (consumed by `TriggerFiltersExtension` in the Rust compiler)

## Type generation

Types in `src/shared/types.gen.ts` are auto-generated from the Rust IR via:

```bash
npm run codegen
```

This invokes `cargo run -- export-gate-schema` to write the JSON Schema, then runs `json-schema-to-typescript`. CI verifies the generated file is up to date (drift check). If drift is detected, run `npm run codegen` and commit the result.

## Layout

- `src/shared/` — modules shared across all bundles (auth, ado-client, vso-logger, env-facts, policy state machine)
- `src/gate/` — gate evaluator entry point and per-concern modules
- `dist/` — ncc bundle output (gitignored); `npm run build` copies the gate bundle to `../gate.js` for inclusion in `scripts.zip`

## See also

- Plan: `ado-script-design.md` at the repo root.
- Compiler integration: `src/compile/extensions/trigger_filters.rs`.
