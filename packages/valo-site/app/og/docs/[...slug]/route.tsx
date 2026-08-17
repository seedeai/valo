import { ogImage } from '@/lib/og-image';
import { getPageImageUrl, source } from '@/lib/source';
import { notFound } from 'next/navigation';

export function generateStaticParams() {
  return source.getPages().map((page) => ({
    slug: getPageImageUrl(page).segments,
  }));
}

export async function GET(_request: Request, { params }: RouteContext<'/og/docs/[...slug]'>) {
  const { slug } = await params;
  const page = source.getPage(slug.slice(0, -1));
  if (!page) notFound();

  return ogImage({
    title: page.data.title,
    description: page.data.description,
  });
}
