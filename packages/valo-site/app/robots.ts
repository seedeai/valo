import type { MetadataRoute } from 'next';
import { siteOrigin } from '@/lib/shared';

export default function robots(): MetadataRoute.Robots {
  const origin = siteOrigin();
  return {
    rules: { userAgent: '*', allow: '/' },
    sitemap: `${origin}/sitemap.xml`,
  };
}
