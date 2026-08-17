import type { MetadataRoute } from 'next';
import { appName, brand, description, tagline } from '@/lib/shared';

export default function manifest(): MetadataRoute.Manifest {
  return {
    name: appName,
    short_name: appName,
    description: `${tagline} ${description}`,
    start_url: '/',
    display: 'standalone',
    background_color: brand.ink,
    theme_color: brand.ink,
    icons: [
      { src: '/icon.png', sizes: '96x96', type: 'image/png', purpose: 'any' },
      { src: '/apple-icon', sizes: '180x180', type: 'image/png' },
    ],
  };
}
