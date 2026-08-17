import { ogImage } from '@/lib/og-image';
import { tagline } from '@/lib/shared';

export const alt = `valo — ${tagline}`;
export const size = { width: 1200, height: 630 };
export const contentType = 'image/png';

export default function OpenGraphImage() {
  return ogImage({ title: 'The WebGPU render engine.', description: tagline });
}
