import { Hero } from '@/components/home/hero';
import { Features } from '@/components/home/features';
import { getHomeContent } from '@/lib/home-content';

export default async function HomePage({ params }: { params: Promise<{ lang: string }> }) {
  const { lang } = await params;
  const content = getHomeContent(lang);

  return (
    <main className="flex flex-1 flex-col">
      <Hero locale={lang} content={content} />
      <Features content={content} />
    </main>
  );
}
