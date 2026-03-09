import type { SidebarsConfig } from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  documentationSidebar: [
    'index',
    'quickstart',
    {
      type: 'category',
      label: 'Recipes',
      items: [
        'recipes/README',
        'recipes/linear-pipeline',
        'recipes/branching',
        'recipes/fan-out',
        'recipes/fan-in',
        'recipes/fan-out-finally',
        'recipes/hooks',
        'recipes/validation',
        'recipes/retry',
        'recipes/commands',
        'recipes/nested-gsd',
      ],
    },
    {
      type: 'category',
      label: 'Reference',
      items: [
        'reference/cli',
        'reference/config-schema',
      ],
    },
    'roadmap',
  ],
};

export default sidebars;
