import { defineConfig } from 'fumadocs-mdx/config';
import rehypeKatex from 'rehype-katex';
import remarkMath from 'remark-math';

export default defineConfig({
  mdxOptions: {
    remarkPlugins: [remarkMath],
    // KaTeX must run before the syntax highlighter.
    rehypePlugins: (v) => [rehypeKatex, ...v],
  },
});
