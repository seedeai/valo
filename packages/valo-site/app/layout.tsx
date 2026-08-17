import { RootProvider } from 'fumadocs-ui/provider/next';
import { DM_Mono, Manrope } from 'next/font/google';
import type { Metadata, Viewport } from 'next';
import './global.css';
import {
  appName,
  brand,
  description,
  keywords,
  siteOrigin,
  tagline,
} from '@/lib/shared';

const manrope = Manrope({
  subsets: ['latin'],
  weight: ['400', '500', '600'],
  variable: '--font-manrope',
});

const dmMono = DM_Mono({
  subsets: ['latin'],
  weight: ['400', '500'],
  variable: '--font-dm-mono',
});

const origin = siteOrigin();
const title = `${appName} — ${tagline}`;

export const viewport: Viewport = {
  themeColor: brand.ink,
  colorScheme: 'dark',
};

export const metadata: Metadata = {
  metadataBase: new URL(origin),
  title: { default: title, template: `%s — ${appName}` },
  description,
  applicationName: appName,
  keywords,
  authors: [{ name: appName, url: origin }],
  creator: appName,
  category: 'technology',
  alternates: { canonical: '/' },
  openGraph: {
    type: 'website',
    locale: 'en_US',
    url: origin,
    siteName: appName,
    title,
    description,
  },
  twitter: {
    card: 'summary_large_image',
    title,
    description,
  },
  icons: {
    icon: [
      { url: '/favicon.svg', type: 'image/svg+xml' },
      { url: '/icon.png', type: 'image/png', sizes: '96x96' },
    ],
    apple: [{ url: '/apple-icon', sizes: '180x180', type: 'image/png' }],
  },
  robots: { index: true, follow: true },
};

export default function Layout({ children }: LayoutProps<'/'>) {
  return (
    <html
      lang="en"
      // One theme, fixed. `suppressHydrationWarning` is still required because
      // the provider writes to `documentElement` before React hydrates.
      className={`dark ${manrope.variable} ${dmMono.variable}`}
      suppressHydrationWarning
    >
      <body className="flex min-h-screen flex-col font-sans">
        <RootProvider theme={{ enabled: false, forcedTheme: 'dark', defaultTheme: 'dark' }}>
          {children}
        </RootProvider>
      </body>
    </html>
  );
}
