import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';
import { Brand } from '@/components/brand';
import { repositoryUrl, rustDocsUrl } from './shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: <Brand />,
    },
    links: [
      { text: 'Docs', url: '/docs', active: 'nested-url' },
      { text: 'Playground', url: '/playground', active: 'url' },
      { text: 'Rust API', url: rustDocsUrl, external: true },
      { text: 'llms.txt', url: '/llms.txt', external: true },
    ],
    githubUrl: repositoryUrl,
    // One theme, so there is nothing for a switcher to switch.
    themeSwitch: { enabled: false },
  };
}
