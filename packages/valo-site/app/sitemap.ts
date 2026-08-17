import type { MetadataRoute } from 'next';
import { siteOrigin } from '@/lib/shared';
import { source } from '@/lib/source';

export default function sitemap(): MetadataRoute.Sitemap {
  const origin = siteOrigin();
  const docs = source.getPages().map((page) => ({
    url: `${origin}${page.url}`,
    changeFrequency: 'weekly' as const,
    priority: 0.7,
  }));

  return [
    { url: origin, changeFrequency: 'weekly', priority: 1 },
    { url: `${origin}/playground`, changeFrequency: 'monthly', priority: 0.8 },
    ...docs,
  ];
}
