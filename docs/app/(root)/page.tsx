'use client';
import { useEffect } from 'react';
import { locales, type Locale } from '@/lib/i18n';

const localeNames: Record<Locale, string> = {
  en: 'English',
  zh: '简体中文',
  'zh-TW': '繁體中文',
  ja: '日本語',
};

function detectLocale(): Locale {
  const candidates = navigator.languages ?? [navigator.language];
  for (const raw of candidates) {
    const tag = raw.toLowerCase();
    if (tag.startsWith('ja')) return 'ja';
    if (tag.startsWith('zh')) {
      if (
        tag.includes('tw') ||
        tag.includes('hant') ||
        tag.includes('hk') ||
        tag.includes('mo')
      ) {
        return 'zh-TW';
      }
      return 'zh';
    }
    if (tag.startsWith('en')) return 'en';
  }
  return 'en';
}

/**
 * Static-export root: there is no server middleware, so locale
 * negotiation happens on the client. Host-level redirects in
 * `vercel.json` / `public/_redirects` cover the no-JS case.
 */
export default function RootRedirectPage() {
  useEffect(() => {
    window.location.replace(`/${detectLocale()}`);
  }, []);

  return (
    <main className="flex flex-col items-center justify-center gap-4 flex-1">
      <p className="text-fd-muted-foreground text-sm">Monoize Docs</p>
      <ul className="flex flex-row flex-wrap gap-4">
        {locales.map((locale) => (
          <li key={locale}>
            <a className="font-medium underline" href={`/${locale}`}>
              {localeNames[locale]}
            </a>
          </li>
        ))}
      </ul>
    </main>
  );
}
