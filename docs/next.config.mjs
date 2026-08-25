import { createMDX } from 'fumadocs-mdx/next';

const withMDX = createMDX();

/** @type {import('next').NextConfig} */
const config = {
  output: 'export',
  reactStrictMode: true,
  // Static export has no image-optimizer endpoint; serve imported images as-is.
  images: { unoptimized: true },
};

export default withMDX(config);
