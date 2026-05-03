/**
 * Typed emitters for ADO `##vso[...]` logging commands.
 *
 * Reference: https://learn.microsoft.com/en-us/azure/devops/pipelines/scripts/logging-commands
 *
 * All emitters write a single line to stdout terminated by a newline.
 * Escape semantics: `\r`, `\n`, `]`, `;` are encoded per ADO's
 * logging-command parser so that user-controlled values cannot break
 * out of the command. Property values escape `\r`, `\n`, `]`, `;`.
 * The message body (after the closing `]`) escapes `%`, `\r`, and `\n`.
 */
function escapeProperty(value: string): string {
  return value
    .replace(/%/g, "%25")
    .replace(/\r/g, "%0D")
    .replace(/\n/g, "%0A")
    .replace(/]/g, "%5D")
    .replace(/;/g, "%3B");
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

export function setOutput(name: string, value: string): void {
  const safeName = escapeProperty(name);
  const safeValue = escapeProperty(value);
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

export function complete(result: CompleteResult, msg?: string): void {
  emit(`##vso[task.complete result=${result};]${escapeMessage(msg ?? "done")}`);
}
