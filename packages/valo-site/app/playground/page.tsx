import type { Metadata } from 'next';
import { Playground } from '@/components/playground/playground';

export const metadata: Metadata = {
  title: 'Playground',
  description: "Edit a valo scene against the engine's raw API and watch it re-render live.",
  alternates: { canonical: '/playground' },
};

export default async function PlaygroundPage(props: PageProps<'/playground'>) {
  const search = await props.searchParams;
  const demo = search.demo;
  return <Playground initialDemo={typeof demo === 'string' ? demo : undefined} />;
}
