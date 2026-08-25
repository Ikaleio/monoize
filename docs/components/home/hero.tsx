import Link from 'next/link';
import Image from 'next/image';
import { ArrowRight, BookOpen } from 'lucide-react';
import { Button } from '@/components/ui/button';
import type { HomeContent } from '@/lib/home-content';

export function Hero({ locale, content }: { locale: string; content: HomeContent }) {
  return (
    <section className="relative overflow-hidden">
      <div aria-hidden className="monoize-grid absolute inset-0 -z-10" />
      <div className="mx-auto flex w-full max-w-3xl flex-col items-center gap-6 px-4 py-20 text-center sm:py-28">
        <Image src="/monoize.svg" alt="Monoize logo" width={72} height={72} priority />
        <h1 className="font-display text-4xl font-semibold tracking-tight text-balance sm:text-5xl">
          {content.tagline}
        </h1>
        <p className="max-w-2xl text-muted-foreground text-pretty">{content.description}</p>
        <div className="flex flex-wrap items-center justify-center gap-3">
          <Button asChild size="lg">
            <Link href={`/${locale}/docs/quick-start`}>
              {content.getStarted}
              <ArrowRight aria-hidden />
            </Link>
          </Button>
          <Button asChild size="lg" variant="outline">
            <Link href={`/${locale}/docs`}>
              <BookOpen aria-hidden />
              {content.readDocs}
            </Link>
          </Button>
        </div>
      </div>
    </section>
  );
}
