import { defineCollection, z } from 'astro:content';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    schema: docsSchema({
      extend: z.object({
        'disable-agentic-editing': z.boolean().optional().describe(
          'Prevents AI agents from making automated edits to this page'
        ),
      }),
    }),
  }),
};
