import { ArrowLeftRight, ScanSearch, ShieldCheck, SlidersHorizontal } from 'lucide-react';
import { Card, CardDescription, CardHeader, CardTitle } from '@/components/ui/card';
import type { HomeContent } from '@/lib/home-content';

const icons = [ArrowLeftRight, ScanSearch, SlidersHorizontal, ShieldCheck];

export function Features({ content }: { content: HomeContent }) {
  return (
    <section className="mx-auto w-full max-w-5xl px-4 pb-20">
      <div className="grid grid-cols-1 gap-4 sm:grid-cols-2">
        {content.features.map((feature, index) => {
          const Icon = icons[index];
          return (
            <Card key={feature.title}>
              <CardHeader>
                <div className="mb-1 flex size-9 items-center justify-center rounded-md border bg-background text-primary">
                  <Icon className="size-4" aria-hidden />
                </div>
                <CardTitle className="font-display text-base">{feature.title}</CardTitle>
                <CardDescription className="leading-relaxed">
                  {feature.description}
                </CardDescription>
              </CardHeader>
            </Card>
          );
        })}
      </div>
    </section>
  );
}
