/**
 * Wiki safe-output scenarios: create-wiki-page, update-wiki-page.
 *
 * These require a wiki to exist in the project. Setup discovers the first wiki
 * via REST and SkipErrors when none exists (so an environment without a wiki
 * does not fail the suite). Pages are created under a deterministic
 * `/ado-aw-det/<buildId>/...` path and deleted in cleanup.
 *
 * Test-harness module; not shipped in `ado-script.zip`.
 */
import type { Scenario, ScenarioContext } from "../scenario.js";
import { SkipError } from "../scenario.js";
import { detBody } from "./common.js";

interface WikiState {
  wiki: string;
  path: string;
}

async function discoverWiki(ctx: ScenarioContext, tool: string): Promise<string> {
  const explicit = process.env.E2E_WIKI_NAME?.trim();
  if (explicit) return explicit;
  const wikis = await ctx.rest.listWikis();
  if (wikis.length === 0) {
    throw new SkipError(`${tool}: no wiki exists in the project (set E2E_WIKI_NAME to override)`);
  }
  return wikis[0]!.name;
}

function pagePath(ctx: ScenarioContext, tool: string): string {
  return `/ado-aw-det/${ctx.buildId}/${tool}`;
}

export const createWikiPage: Scenario<WikiState> = {
  tool: "create-wiki-page",
  config: (_ctx, state) => ({ "wiki-name": state.wiki, "include-stats": false }),
  setup: async (ctx) => {
    const wiki = await discoverWiki(ctx, "create-wiki-page");
    return { wiki, path: pagePath(ctx, "create-wiki-page") };
  },
  ndjson: async (ctx, state) => ({
    path: state.path,
    content: detBody(ctx, "create-wiki-page"),
    comment: "deterministic executor e2e create",
  }),
  assert: async (ctx, state) => {
    const page = await ctx.rest.getWikiPage(state.wiki, state.path);
    if (!page) throw new Error(`wiki page '${state.path}' was not created`);
  },
  cleanup: async (ctx, state) => ctx.rest.deleteWikiPage(state.wiki, state.path),
};

export const updateWikiPage: Scenario<WikiState> = {
  tool: "update-wiki-page",
  config: (_ctx, state) => ({ "wiki-name": state.wiki, "include-stats": false }),
  setup: async (ctx) => {
    const wiki = await discoverWiki(ctx, "update-wiki-page");
    const path = pagePath(ctx, "update-wiki-page");
    // Precondition: the page must already exist for update-wiki-page.
    await ctx.rest.putWikiPage(wiki, path, "original deterministic content");
    return { wiki, path };
  },
  ndjson: async (ctx, state) => ({
    path: state.path,
    content: `${detBody(ctx, "update-wiki-page")} (updated)`,
    comment: "deterministic executor e2e update",
  }),
  assert: async (ctx, state) => {
    const page = await ctx.rest.getWikiPage(state.wiki, state.path);
    if (!page?.content?.includes("(updated)")) {
      throw new Error(`wiki page '${state.path}' content was not updated`);
    }
  },
  cleanup: async (ctx, state) => ctx.rest.deleteWikiPage(state.wiki, state.path),
};

export const wikiScenarios: Scenario<any>[] = [createWikiPage, updateWikiPage];
