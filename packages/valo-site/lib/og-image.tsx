import { ImageResponse } from 'next/og';
import { appName, brand, tagline } from './shared';

/** Open Graph size. Google's favicon hint is a multiple of 48; OG is 1200×630. */
export const ogSize = { width: 1200, height: 630 };

/**
 * The nav mark as inline styles ImageResponse can paint.
 *
 * Same geometry as `.valo-mark`: a lime square with three circular corners
 * and one tighter corner, rotated −45°. The lift is a small percentage so a
 * 96px mark still fits inside the 180px apple-touch icon.
 */
export function BrandMark({ size }: { size: number }) {
  return (
    <div
      style={{
        display: 'flex',
        width: size,
        height: size,
        background: brand.lime,
        borderRadius: '50% 50% 50% 12%',
        transform: 'translateY(-8%) rotate(-45deg)',
      }}
    />
  );
}

export function ogImage({ title, description }: { title: string; description?: string }) {
  return new ImageResponse(
    (
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          justifyContent: 'space-between',
          width: '100%',
          height: '100%',
          padding: 72,
          background: brand.ink,
          color: brand.text,
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', gap: 20 }}>
          <BrandMark size={48} />
          <span style={{ fontSize: 36, fontWeight: 600, letterSpacing: '-0.04em' }}>{appName}</span>
        </div>
        <div style={{ display: 'flex', flexDirection: 'column', gap: 18 }}>
          <span
            style={{
              fontSize: title.length > 28 ? 56 : 72,
              fontWeight: 500,
              letterSpacing: '-0.05em',
              lineHeight: 1.05,
            }}
          >
            {title}
          </span>
          <span style={{ fontSize: 28, color: brand.muted, lineHeight: 1.35 }}>
            {description ?? tagline}
          </span>
        </div>
      </div>
    ),
    { ...ogSize },
  );
}
