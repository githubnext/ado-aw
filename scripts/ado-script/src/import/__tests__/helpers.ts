import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { ModuleKind, ScriptTarget, transpileModule } from "typescript";

const __dirname = dirname(fileURLToPath(import.meta.url));
const sourceEntryPath = resolve(__dirname, "../index.ts");
const scratchRoot = resolve(__dirname, ".scratch");

export type RunResult = {
  stdout: string;
  stderr: string;
  status: number | null;
};

function sanitizeLabel(label: string): string {
  return label.replace(/[^a-z0-9]+/gi, "-").replace(/^-+|-+$/g, "").toLowerCase() || "case";
}

export function withScratchDir(label: string, run: (dir: string) => void): void {
  const dir = resolve(scratchRoot, `${sanitizeLabel(label)}-${randomUUID()}`);
  mkdirSync(dir, { recursive: true });

  try {
    run(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

export function writeFixture(baseDir: string, relativePath: string, contents: string): string {
  const filePath = resolve(baseDir, relativePath);
  mkdirSync(dirname(filePath), { recursive: true });
  writeFileSync(filePath, contents, "utf8");
  return filePath;
}

export function readText(filePath: string): string {
  return readFileSync(filePath, "utf8");
}

export type RunOptions = {
  env?: NodeJS.ProcessEnv;
  /** Optional `--base <path>` argument forwarded to `import.js`. */
  base?: string;
};

export function runImportSource(target: string, options: RunOptions = {}): RunResult {
  const runnerPath = resolve(dirname(target), "__runtime-import-runner.mjs");
  const compiled = transpileModule(readFileSync(sourceEntryPath, "utf8"), {
    compilerOptions: {
      module: ModuleKind.ES2022,
      target: ScriptTarget.ES2022,
    },
  }).outputText;

  writeFileSync(runnerPath, compiled, "utf8");

  const args = [runnerPath, target];
  if (options.base !== undefined) {
    args.push("--base", options.base);
  }

  const result = spawnSync(process.execPath, args, {
    env: { ...process.env, ...(options.env ?? {}) },
    encoding: "utf8",
  });

  return {
    stdout: result.stdout ?? "",
    stderr: result.stderr ?? "",
    status: result.status,
  };
}
