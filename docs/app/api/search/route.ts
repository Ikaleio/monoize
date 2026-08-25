import { source } from '@/lib/source';
import { createFromSource } from 'fumadocs-core/search/server';

export const revalidate = false;

// The default `multilingual` tokenizer covers en, zh, zh-TW, and ja
// with a single static index.
export const { staticGET: GET } = createFromSource(source);
