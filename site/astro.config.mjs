import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import sitemap from '@astrojs/sitemap';
import starlightLlmsTxt from 'starlight-llms-txt';
import starlightLinksValidator from 'starlight-links-validator';
import mermaid from 'astro-mermaid';

export default defineConfig({
  site: 'https://githubnext.github.io',
  base: '/ado-aw/',
  trailingSlash: 'always',
  integrations: [
    mermaid({ autoTheme: true }),
    starlight({
      title: 'ado-aw',
      description: 'Compile natural-language markdown into Azure DevOps agentic pipelines',
      plugins: [
        starlightLlmsTxt(),
        starlightLinksValidator(),
      ],
      customCss: ['./src/styles/custom.css'],
      components: {
        Head: './src/components/CustomHead.astro',
      },
      tableOfContents: { minHeadingLevel: 2, maxHeadingLevel: 4 },
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/githubnext/ado-aw' },
      ],
      sidebar: [
        {
          label: 'Introduction',
          autogenerate: { directory: 'introduction' },
        },
        {
          label: 'Setup',
          items: [
            { label: 'Quick Start', slug: 'setup/quick-start' },
            { label: 'Service Connections', slug: 'setup/service-connections' },
            { label: 'CLI Commands', slug: 'setup/cli' },
            { label: 'Local Development', slug: 'setup/local-development' },
          ],
        },
        {
          label: 'Guides',
          items: [
            { label: 'Creating Agents', slug: 'guides/creating-agents' },
            { label: 'Example Agents', slug: 'guides/examples' },
            { label: 'Network Configuration', slug: 'guides/network-config' },
            { label: 'Schedule Syntax', slug: 'guides/schedule-syntax' },
            { label: 'Using MCP', slug: 'guides/using-mcp' },
            { label: 'Extending the Compiler', slug: 'guides/extending' },
          ],
        },
        {
          label: 'Reference',
          items: [
            { label: 'Front Matter', slug: 'reference/front-matter' },
            { label: 'Engine', slug: 'reference/engine' },
            { label: 'Parameters', slug: 'reference/parameters' },
            { label: 'Tools', slug: 'reference/tools' },
            { label: 'Runtimes', slug: 'reference/runtimes' },
            { label: 'Safe Outputs', slug: 'reference/safe-outputs' },
            { label: 'ado-aw-debug', slug: 'reference/ado-aw-debug' },
            { label: 'Targets', slug: 'reference/targets' },
            { label: 'Network', slug: 'reference/network' },
            { label: 'MCP', slug: 'reference/mcp' },
            { label: 'MCP Gateway', slug: 'reference/mcpg' },
            { label: 'Template Markers', slug: 'reference/template-markers' },
            { label: 'Runtime Imports', slug: 'reference/runtime-imports' },
            { label: 'Filter IR', slug: 'reference/filter-ir' },
            { label: 'ado-script', slug: 'reference/ado-script' },
            { label: 'Codemods', slug: 'reference/codemods' },
            { label: 'Audit', slug: 'reference/audit' },
          ],
        },
        {
          label: 'Troubleshooting',
          autogenerate: { directory: 'troubleshooting' },
        },
      ],
    }),
    sitemap(),
  ],
});
