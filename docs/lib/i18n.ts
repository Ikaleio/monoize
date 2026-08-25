import { defineI18n } from 'fumadocs-core/i18n';

export const i18n = defineI18n({
  defaultLanguage: 'en',
  languages: ['en', 'zh', 'zh-TW', 'ja'],
});

export type Locale = (typeof i18n)['languages'][number];

export const locales = i18n.languages;
