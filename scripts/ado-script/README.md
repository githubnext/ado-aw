# @ado-aw/scripts

Bundled TypeScript scripts shipped in `ado-script.zip` alongside the ado-aw release.

## Bundles

- `gate.js` — trigger filter gate evaluator (consumed by `AdoScriptExtension::setup_steps` in the Rust compiler)
- `import.js` — runtime `{{#runtime-import …}}` resolver (consumed by `AdoScriptExtension::prepare_steps` in the Rust compiler)

## Type generation

Types in `src/shared/types.gen.ts` are auto-generated from the Rust IR via:

```bash
npm run codegen
```

This invokes `cargo run -- export-gate-schema` to write the JSON Schema, then runs `json-schema-to-typescript`. CI verifies the generated file is up to date (drift check). If drift is detected, run `npm run codegen` and commit the result.

## Layout

- `src/shared/` — modules shared across all bundles (auth, ado-client, vso-logger, env-facts, policy state machine)
- `src/gate/` — gate evaluator entry point and per-concern modules
- `src/import/` — runtime-import resolver entry point
- `dist/` — ncc bundle output (gitignored); `npm run build` writes `dist/gate/index.js` and `dist/import/index.js`, both of which ship in `ado-script.zip`

## See also

- Architecture and runtime contract: [`docs/ado-script.md`](../../docs/ado-script.md).
- Compiler integration: `src/compile/extensions/ado_script.rs`.
