import type { ReactNode } from 'react';
import { Provider } from '@/components/provider';
import { i18n } from '@/lib/i18n';
import '../global.css';
import 'katex/dist/katex.css';

export function generateStaticParams() {
  return i18n.languages.map((lang) => ({ lang }));
}

export default async function Layout({
  params,
  children,
}: {
  params: Promise<{ lang: string }>;
  children: ReactNode;
}) {
  const { lang } = await params;

  return (
    <html lang={lang} suppressHydrationWarning>
      <body className="flex flex-col min-h-screen">
        <Provider locale={lang}>{children}</Provider>
      </body>
    </html>
  );
}
