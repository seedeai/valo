import { RootProvider } from 'fumadocs-ui/provider/next';
import { DM_Mono, Manrope } from 'next/font/google';
import type { Metadata } from 'next';
import './global.css';
import { appName, tagline } from '@/lib/shared';

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

export const metadata: Metadata = {
  metadataBase: new URL(process.env.NEXT_PUBLIC_SITE_URL ?? 'https://valo.im'),
  title: { default: `${appName} — ${tagline}`, template: `%s — ${appName}` },
  description:
    'A WebGPU-native 2D render engine for Rust on native platforms and WebAssembly, with JavaScript, TypeScript and Canvas2D-compatible browser APIs.',
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
