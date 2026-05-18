/**
 * Typed emitters for ADO `##vso[...]` logging commands.
 *
 * Reference: https://learn.microsoft.com/en-us/azure/devops/pipelines/scripts/logging-commands
 *
 * All emitters write a single line to stdout terminated by a newline.
 * Escape semantics: `\r`, `\n`, `]`, `;` are encoded per ADO's
 * logging-command parser so that user-controlled values cannot break
 * out of the command. Property values additionally encode `=` and SPACE
 * because ADO's command-header parser tokenises `key=value` pairs on
 * whitespace and `=`; without this an adversarial property value
 * containing either would smuggle a new key into the command header.
 * The message body (after the closing `]`) escapes `%`, `\r`, and `\n`.
 */
function escapeProperty(value: string): string {
  return value
    .replace(/%/g, "%25")
    .replace(/\r/g, "%0D")
    .replace(/\n/g, "%0A")
    .replace(/]/g, "%5D")
    .replace(/;/g, "%3B")
    .replace(/=/g, "%3D")
    .replace(/ /g, "%20");
}

function escapeMessage(value: string): string {
  return value
    .replace(/%/g, "%25")
    .replace(/\r/g, "%0D")
    .replace(/\n/g, "%0A");
}

function emit(line: string): void {
  process.stdout.write(line + "\n");
}

/** Generic emitter for callers that need to write something visible to the
 *  pipeline log without using one of the structured `task.logissue` or
 *  `task.complete` shapes. The message is escaped the same way as the body
 *  of a `##vso` command, AND a leading `#` is percent-encoded so an
 *  adversarial message cannot smuggle a `##vso[` command (ADO only
 *  interprets `##vso[` at line-start). */
export function logInfo(msg: string): void {
  const safe = escapeMessage(msg).replace(/^#/, "%23");
  emit(safe);
}

export function setOutput(name: string, value: string): void {
  const safeName = escapeProperty(name);
  const safeValue = escapeMessage(value);
  emit(`##vso[task.setvariable variable=${safeName};isOutput=true]${safeValue}`);
}

export function addBuildTag(tag: string): void {
  emit(`##vso[build.addbuildtag]${escapeMessage(tag)}`);
}

export function logWarning(msg: string): void {
  emit(`##vso[task.logissue type=warning;]${escapeMessage(msg)}`);
}

export function logError(msg: string): void {
  emit(`##vso[task.logissue type=error;]${escapeMessage(msg)}`);
}

export type CompleteResult = "Succeeded" | "Failed" | "SucceededWithIssues";

// `complete()` is idempotent: ADO's behaviour on two consecutive
// `##vso[task.complete]` commands is undefined (some runners ignore the
// second, others let it override). We track first-call winning so the
// runtime contract is unambiguous regardless of caller composition
// (e.g. bypass returning early then main also reaching the final emit).
let completed = false;

export function complete(result: CompleteResult, msg?: string): void {
  if (completed) return;
  completed = true;
  emit(`##vso[task.complete result=${result};]${escapeMessage(msg ?? "done")}`);
}

/** For tests only: clear the `complete()` latch between cases. */
export function _resetCompletedForTesting(): void {
  completed = false;
}
