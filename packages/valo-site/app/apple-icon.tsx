import { ImageResponse } from 'next/og';
import { BrandMark } from '@/lib/og-image';
import { brand } from '@/lib/shared';

export const size = { width: 180, height: 180 };
export const contentType = 'image/png';

export default function AppleIcon() {
  return new ImageResponse(
    (
      <div
        style={{
          display: 'flex',
          width: '100%',
          height: '100%',
          alignItems: 'center',
          justifyContent: 'center',
          background: brand.ink,
        }}
      >
        <BrandMark size={96} />
      </div>
    ),
    { ...size },
  );
}
