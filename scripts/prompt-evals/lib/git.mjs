import { readFile } from "node:fs/promises";
import path from "node:path";

import { runProcess } from "./process.mjs";

export async function gitOutput(repoRoot, args, options = {}) {
  const result = await runProcess("git", args, {
    cwd: repoRoot,
    timeoutMs: options.timeoutMs ?? 120_000,
    maxOutputBytes: options.maxOutputBytes ?? 1_048_576
  });
  if (!result.success) {
    throw new Error(
      `git ${args.join(" ")} failed: ${result.stderr || result.stdout}`
    );
  }
  return result.stdout;
}

export async function changedFiles(repoRoot, baseSha, headSha) {
  const output = await gitOutput(repoRoot, [
    "diff",
    "--name-only",
    "--no-renames",
    baseSha,
    headSha,
    "--"
  ]);
  return output
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
}

export async function fileAtRef(repoRoot, ref, relativePath) {
  const result = await runProcess(
    "git",
    ["show", `${ref}:${relativePath.replaceAll("\\", "/")}`],
    {
      cwd: repoRoot,
      timeoutMs: 120_000,
      maxOutputBytes: 2_097_152
    }
  );
  if (result.success) {
    return result.stdout;
  }
  if (
    result.stderr.includes("does not exist") ||
    result.stderr.includes("exists on disk, but not in")
  ) {
    return null;
  }
  throw new Error(
    `git show ${ref}:${relativePath} failed: ${result.stderr || result.stdout}`
  );
}

export async function currentFile(repoRoot, relativePath) {
  return readFile(path.join(repoRoot, relativePath), "utf8");
}

export async function currentSha(repoRoot) {
  return (await gitOutput(repoRoot, ["rev-parse", "HEAD"])).trim();
}

