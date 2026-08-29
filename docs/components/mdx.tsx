import defaultMdxComponents from 'fumadocs-ui/mdx';
import { Heading } from 'fumadocs-ui/components/heading';
import type { MDXComponents } from 'mdx/types';
import type { ComponentPropsWithoutRef } from 'react';

import { cn } from '@/lib/utils';

type HeadingTag = 'h1' | 'h2' | 'h3' | 'h4' | 'h5' | 'h6';

function AlignedHeading({
  as,
  className,
  ...props
}: ComponentPropsWithoutRef<'h1'> & { as: HeadingTag }) {
  return (
    <Heading
      as={as}
      className={cn('items-start [&>button]:mt-1', className)}
      {...props}
    />
  );
}

const alignedHeadings = {
  h1: (props) => <AlignedHeading as="h1" {...props} />,
  h2: (props) => <AlignedHeading as="h2" {...props} />,
  h3: (props) => <AlignedHeading as="h3" {...props} />,
  h4: (props) => <AlignedHeading as="h4" {...props} />,
  h5: (props) => <AlignedHeading as="h5" {...props} />,
  h6: (props) => <AlignedHeading as="h6" {...props} />,
} satisfies Pick<MDXComponents, HeadingTag>;

export function getMDXComponents(components?: MDXComponents) {
  return {
    ...defaultMdxComponents,
    ...alignedHeadings,
    ...components,
  } satisfies MDXComponents;
}

export const useMDXComponents = getMDXComponents;

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}
