import { source } from '@/lib/source';
import { llms } from 'fumadocs-core/source';

export const revalidate = false;

export function GET() {
  return new Response(llms(source).index(), {
    headers: { 'Content-Type': 'text/markdown; charset=utf-8' },
  });
}
