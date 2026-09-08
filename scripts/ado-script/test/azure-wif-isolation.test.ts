import { spawnSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";

const testDir = dirname(fileURLToPath(import.meta.url));
const repo = resolve(testDir, "../../..");
const bundle = resolve(testDir, "../azure-wif-refresh.js");
const dockerEnabled = process.env.ADO_AW_TEST_DOCKER === "1";
const awfEnabled = process.env.ADO_AW_TEST_AWF === "1";
const image = "node:20-slim";

interface PipelineStep {
  displayName?: string;
  bash?: string;
  inputs?: { inlineScript?: string };
}

interface Pipeline {
  jobs: Array<{ job: string; steps: PipelineStep[] }>;
}

interface McpgConfig {
  mcpServers: Record<string, { mounts?: string[] }>;
}

function run(command: string, args: string[], env?: NodeJS.ProcessEnv): string {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    timeout: 180_000,
    maxBuffer: 4 * 1024 * 1024,
    env,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${command} exited ${result.status}\n${result.stdout}\n${result.stderr}`);
  }
  return result.stdout.trim();
}

function docker(...args: string[]): string {
  return run("docker", args);
}

function compileFixture(directory: string) {
  const source = join(directory, "workflow.md");
  const output = join(directory, "workflow.yml");
  writeFileSync(source, `---
name: WIF isolation
description: Credential-free isolation regression
mcp-servers:
  fixture:
    container: node:20-slim
    args: [--user, "20001:20001"]
    azure-auth:
      service-connection: unused-test-connection
---
Do not invoke an agent. This workflow is only compiled by the test.
`);
  const binary = resolve(repo, `target/debug/ado-aw${process.platform === "win32" ? ".exe" : ""}`);
  run(binary, ["compile", "--force", source, "-o", output]);
  const pipeline: Pipeline = parse(readFileSync(output, "utf8"));
  const start = pipeline.jobs.flatMap((job) => job.steps)
    .find((step) => step.displayName === "Start Azure auth refresher (fixture)");
  const script = start?.inputs?.inlineScript;
  if (!script) throw new Error("compiled Azure WIF startup is missing");
  const launch = script.indexOf("docker run \\\n");
  if (launch < 0) throw new Error("compiled Azure WIF container launch is missing");
  const identities = [...script.matchAll(/^(?:CLIENT|TENANT)_VARIABLE='([^']+)'$/gm)]
    .map((match) => match[1]!);
  expect(identities).toHaveLength(2);
  const configStep = pipeline.jobs.flatMap((job) => job.steps)
    .find((step) => step.displayName === "Prepare MCPG config");
  const configMatch = configStep?.bash?.match(
    /cat > "\$AGENT_TEMP\/staging\/mcpg-config.json" << '([^']+)'\n([\s\S]*?)\n\1/,
  );
  if (!configMatch) throw new Error("compiled MCPG configuration is missing");
  const config: McpgConfig = JSON.parse(configMatch[2]!);
  const mounts = config.mcpServers.fixture?.mounts;
  if (!mounts || mounts.length !== 1) throw new Error("expected one compiler-owned assertion mount");
  return { pipeline, setup: script.slice(0, launch), identities, tokenMount: mounts[0]! };
}

async function until(message: string, predicate: () => boolean): Promise<void> {
  const deadline = Date.now() + 30_000;
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error(message);
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
}

describe.skipIf(!dockerEnabled)("Azure WIF Linux filesystem contract", () => {
  it("rotates through a read-only different-UID mount without exposing private siblings", async () => {
    expect(docker("info", "--format", "{{.OSType}}")).toBe("linux");
    expect(existsSync(bundle), "build:azure-wif-refresh must run first").toBe(true);
    const directory = mkdtempSync(join(tmpdir(), "ado-aw-wif-docker-"));
    const volume = `ado-aw-wif-${randomUUID()}`;
    const producer = `${volume}-producer`;
    const consumer = `${volume}-consumer`;
    let containerCreated = false;
    let consumerCreated = false;
    let volumeCreated = false;
    try {
      const { setup, tokenMount } = compileFixture(directory);
      // Only Docker cleanup is stubbed. Run the compiler's actual credential
      // metadata checks, umask, directory permissions and FIFO creation.
      writeFileSync(join(directory, "setup.sh"), `
docker() { [ "$1" = rm ] || return 1; }
${setup.replaceAll("$(Agent.TempDirectory)", "/state")}
`);
      copyFileSync(bundle, join(directory, "refresher.mjs"));
      copyFileSync(join(testDir, "fixtures/azure-wif-isolation.cjs"), join(directory, "worker.cjs"));
      docker("volume", "create", volume);
      volumeCreated = true;
      docker("create", "--name", producer, "--network", "none",
        "--mount", `type=volume,source=${volume},target=/state`,
        image, "node", "/inputs/worker.cjs");
      containerCreated = true;
      docker("cp", `${directory}${process.platform === "win32" ? "\\." : "/."}`, `${producer}:/inputs`);
      docker("start", producer);
      const exec = (script: string) => docker("exec", producer, "node", "-e", script);
      await until("refresher did not publish readiness", () => {
        const ready = exec(`const f=require("fs");
          if (!f.existsSync("/state/auth-path")) { console.log("pending"); }
          else { const p=f.readFileSync("/state/auth-path","utf8");
            console.log(f.existsSync(p+"/ready.json") ? "ready" : "pending"); }`);
        if (ready === "ready") return true;
        expect(docker("inspect", "-f", "{{.State.Running}}", producer),
          docker("logs", producer)).toBe("true");
        return false;
      });
      const auth = exec('process.stdout.write(require("fs").readFileSync("/state/auth-path","utf8"))');
      const hostRoot = docker("volume", "inspect", "--format", "{{.Mountpoint}}", volume);
      const [mountSource, mountDestination, mountMode] = tokenMount
        .replace("$(Agent.TempDirectory)", hostRoot).split(":");
      if (!mountSource || !mountDestination || !mountMode) {
        throw new Error("malformed compiler-generated assertion mount");
      }
      expect(mountSource).toBe(`${hostRoot}${auth.slice("/state".length)}/token.d`);
      expect(mountDestination).toBe("/var/run/ado-aw/azure");
      expect(mountMode).toBe("ro");
      docker("create", "--name", consumer, "--network", "none",
        "--user", "20001:20001", "--cap-drop", "ALL",
        "-v", `${mountSource}:${mountDestination}:${mountMode}`,
        image, "node", "-e", "setInterval(()=>{},1000)");
      consumerCreated = true;
      docker("start", consumer);
      const readAsConsumer = (script: string) => docker(
        "exec", consumer, "node", "-e", script.replaceAll("/identity", mountDestination),
      );
      const initial = readAsConsumer('process.stdout.write(require("fs").readFileSync("/identity/token","utf8"))');
      exec('require("fs").writeFileSync("/state/advance-1","")');
      await until("refresher did not rotate the assertion", () =>
        exec(`const f=require("fs"); const s=JSON.parse(f.readFileSync(${JSON.stringify(auth + "/status.json")},"utf8")); console.log(s.lastRefreshAt ? "rotated" : "pending")`) === "rotated");
      const replacement = readAsConsumer('process.stdout.write(require("fs").readFileSync("/identity/token","utf8"))');
      expect(replacement).not.toBe(initial);
      expect(JSON.parse(Buffer.from(replacement.split(".")[1]!, "base64url").toString()).exp)
        .toBeGreaterThan(JSON.parse(Buffer.from(initial.split(".")[1]!, "base64url").toString()).exp);
      readAsConsumer(`
        const f=require("fs"),a=require("assert/strict");
        a.equal(f.statSync("/identity/token").mode & 511, 420);
        a.throws(()=>f.writeFileSync("/identity/token","tampered"),{code:"EROFS"});
        for (const p of ["/identity/material","/identity/status.json","/identity/ready.json",
                         "/identity/../material","/identity/../status.json"]) {
          a.throws(()=>f.readFileSync(p),{code:"ENOENT"});
        }`);
      docker("run", "--rm", "--network", "none", "--user", "20001:20001",
        "--cap-drop", "ALL", "--mount", `type=bind,source=${hostRoot},target=/host-view,readonly`,
        image, "node", "-e", `
          const f=require("fs"),a=require("assert/strict");
          a.equal(f.readFileSync("/host-view/public","utf8"),"host-view-control");
          a.throws(()=>f.readFileSync(${JSON.stringify(`/host-view${auth.slice("/state".length)}/token.d/token`)}),{code:"EACCES"});`);
      exec('require("fs").writeFileSync("/state/stop","")');
      expect(docker("wait", producer)).toBe("0");
    } finally {
      if (consumerCreated) docker("rm", "-f", consumer);
      if (containerCreated) docker("rm", "-f", producer);
      if (volumeCreated) docker("volume", "rm", volume);
      rmSync(directory, { recursive: true, force: true });
    }
  }, 180_000);
});

describe.skipIf(!awfEnabled)("Azure WIF real AWF boundary", () => {
  it("hides the host assertion and internal IDs from normal and chroot paths", async () => {
    expect(process.platform, "the real AWF regression requires Linux").toBe("linux");
    if (!process.getuid || !process.getgid) throw new Error("Linux process identity is unavailable");
    const owner = `${process.getuid()}:${process.getgid()}`;
    expect(docker("info", "--format", "{{.OSType}}")).toBe("linux");
    // /tmp is intentionally agent-readable in AWF; keep private material in
    // a sibling of the workspace, outside both /tmp and mounted home subdirs.
    const directory = mkdtempSync(join(homedir(), "ado-aw-wif-awf-"));
    try {
      const { pipeline, identities } = compileFixture(directory);
      const workspace = join(directory, "workspace");
      const temp = join(directory, "runner-temp");
      const tools = join(directory, "tools");
      const home = join(directory, "home");
      const auth = join(temp, "ado-aw-azure-auth", "fixture");
      mkdirSync(workspace, { recursive: true });
      mkdirSync(home);
      mkdirSync(join(auth, "token.d"), { recursive: true });
      chmodSync(join(temp, "ado-aw-azure-auth"), 0o700);
      chmodSync(auth, 0o700);
      chmodSync(join(auth, "token.d"), 0o755);
      writeFileSync(join(auth, "token.d/token"), "synthetic-assertion", { mode: 0o644 });
      symlinkSync(join(auth, "token.d/token"), join(workspace, "token-link"));
      mkdirSync(join(tools, "awf"), { recursive: true });

      const runStep = pipeline.jobs.find((job) => job.job === "Agent")?.steps
        .find((step) => step.bash?.includes("AWF_ARGS+=(--skip-pull --env-all)"));
      if (!runStep?.bash) throw new Error("compiled AWF invocation is missing");
      const capture = join(directory, "awf-args");
      writeFileSync(join(tools, "awf/awf"), `#!/bin/sh
if [ "$1" = logs ]; then exit 0; fi
printf '%s\\0' "$@" > '${capture}'
`, { mode: 0o755 });
      const script = runStep.bash
        .replaceAll("$(Agent.TempDirectory)", temp)
        .replaceAll("$(Pipeline.Workspace)", tools)
        .replaceAll("$(Build.SourcesDirectory)", workspace);
      const env: NodeJS.ProcessEnv = {
        PATH: process.env.PATH,
        HOME: home,
        WORKING_DIRECTORY: workspace,
        WIF_TEST_AUTH: auth,
        WIF_TEST_WORKSPACE: workspace,
      };
      for (const name of identities) env[name] = "synthetic-identity";
      run("bash", ["-c", script], env);
      const captured = readFileSync(capture, "utf8").split("\0").filter(Boolean);
      const version = captured[captured.indexOf("--image-tag") + 1];
      expect(version).toMatch(/^\d+\.\d+\.\d+$/);
      const executable = join(directory, "awf");
      const release = `https://github.com/github/gh-aw-firewall/releases/download/v${version}`;
      const binaryResponse = await fetch(`${release}/awf-linux-x64`);
      if (!binaryResponse.ok) throw new Error(`AWF download: HTTP ${binaryResponse.status}`);
      const bytes = Buffer.from(await binaryResponse.arrayBuffer());
      const checksumsResponse = await fetch(`${release}/checksums.txt`);
      if (!checksumsResponse.ok) throw new Error(`AWF checksums: HTTP ${checksumsResponse.status}`);
      const checksums = await checksumsResponse.text();
      const checksum = checksums.split("\n").find((line) => /\s+\*?awf-linux-x64$/.test(line.trim()));
      expect(checksum, "missing AWF checksum").toBeDefined();
      expect(createHash("sha256").update(bytes).digest("hex")).toBe(checksum!.split(/\s+/)[0]);
      writeFileSync(executable, bytes, { mode: 0o755 });

      const commandIndex = captured.indexOf("--");
      expect(commandIndex).toBeGreaterThan(0);
      const args: string[] = [];
      for (let i = 0; i < commandIndex; i++) {
        // This probe exercises filesystem/env isolation, not MCP networking:
        // omit the absent MCPG peer and allow AWF to pull its pinned images.
        if (captured[i] === "--topology-attach") { i++; continue; }
        if (captured[i] === "--skip-pull") continue;
        args.push(captured[i]!);
      }
      const probe = `set -eu
test "$(cat "$WIF_TEST_WORKSPACE/control")" = workspace-visible
for p in "$WIF_TEST_AUTH/token.d/token" "/host$WIF_TEST_AUTH/token.d/token" "$WIF_TEST_WORKSPACE/token-link"; do
  if cat "$p" >/dev/null 2>&1; then echo "Assertion exposed: $p" >&2; exit 1; fi
done
${identities.map((key) => `if printenv '${key}' >/dev/null; then echo "Internal identity exposed" >&2; exit 1; fi`).join("\n")}
echo wif-isolation-passed
`;
      writeFileSync(join(workspace, "control"), "workspace-visible");
      writeFileSync(join(workspace, "probe.sh"), probe);
      // AWF owns fixed container names. Never let a test replace an unrelated
      // local AWF session; CI runs this probe once on its dedicated runner.
      expect(docker("ps", "-aq", "--filter", "name=awf-"),
        "an existing AWF session must be stopped by its owner first").toBe("");
      const output = run(executable, [
        ...args, "--work-dir", join(directory, "awf-state"),
        "--agent-timeout", "1",
        "--", `bash '${join(workspace, "probe.sh")}'`,
      ], { ...env, GITHUB_WORKSPACE: workspace });
      expect(output).toContain("wif-isolation-passed");
    } finally {
      // AWF's container setup creates root-owned files in its synthetic home.
      // Restore only this disposable tree, without following symlinks.
      docker("run", "--rm", "--network", "none",
        "--mount", `type=bind,source=${directory},target=/fixture`,
        image, "chown", "-Rh", owner, "/fixture");
      rmSync(directory, { recursive: true, force: true });
    }
  }, 240_000);
});
