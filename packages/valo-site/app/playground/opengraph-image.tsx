import { ogImage } from '@/lib/og-image';

export const alt = 'valo playground';
export const size = { width: 1200, height: 630 };
export const contentType = 'image/png';

export default function OpenGraphImage() {
  return ogImage({
    title: 'Playground',
    description: "Edit a valo scene against the engine's raw API and watch it re-render live.",
  });
}
