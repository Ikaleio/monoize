import { motion, type Variants, type Transition } from "framer-motion";
import { forwardRef, type ReactNode, type ComponentProps } from "react";

// Easing functions - non-linear for smooth feel
export const easings = {
  easeOutExpo: [0.16, 1, 0.3, 1],
  easeOutQuart: [0.25, 1, 0.5, 1],
  easeOutBack: [0.34, 1.56, 0.64, 1],
  easeInOutQuart: [0.76, 0, 0.24, 1],
  spring: { type: "spring", stiffness: 300, damping: 30 },
} as const;

// Common transitions
export const transitions = {
  fast: { duration: 0.15, ease: easings.easeOutExpo },
  normal: { duration: 0.25, ease: easings.easeOutExpo },
  slow: { duration: 0.4, ease: easings.easeOutExpo },
  spring: { type: "spring", stiffness: 400, damping: 30 },
  springBounce: { type: "spring", stiffness: 300, damping: 20 },
} as const;

// Page transition variants
export const pageVariants: Variants = {
  initial: { opacity: 0, y: 12 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: -12 },
};

// Fade variants
export const fadeVariants: Variants = {
  initial: { opacity: 0 },
  animate: { opacity: 1 },
  exit: { opacity: 0 },
};

// Scale fade variants
export const scaleFadeVariants: Variants = {
  initial: { opacity: 0, scale: 0.95 },
  animate: { opacity: 1, scale: 1 },
  exit: { opacity: 0, scale: 0.95 },
};

// Slide up variants
export const slideUpVariants: Variants = {
  initial: { opacity: 0, y: 20 },
  animate: { opacity: 1, y: 0 },
  exit: { opacity: 0, y: 20 },
};

// Stagger container variants
export const staggerContainerVariants: Variants = {
  initial: {},
  animate: {
    transition: {
      staggerChildren: 0.05,
      delayChildren: 0.1,
    },
  },
};

// Stagger item variants
export const staggerItemVariants: Variants = {
  initial: { opacity: 0, y: 10 },
  animate: { opacity: 1, y: 0 },
};

// Page wrapper component
interface PageWrapperProps {
  children: ReactNode;
  className?: string;
}

export const PageWrapper = forwardRef<HTMLDivElement, PageWrapperProps>(
  ({ children, className = "" }, ref) => (
    <motion.div
      ref={ref}
      initial="initial"
      animate="animate"
      exit="exit"
      variants={pageVariants}
      transition={transitions.normal}
      className={className}
    >
      {children}
    </motion.div>
  )
);
PageWrapper.displayName = "PageWrapper";

// Fade in component
interface FadeInProps {
  children: ReactNode;
  className?: string;
  delay?: number;
}

export const FadeIn = forwardRef<HTMLDivElement, FadeInProps>(
  ({ children, className = "", delay = 0 }, ref) => (
    <motion.div
      ref={ref}
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      transition={{ ...transitions.normal, delay }}
      className={className}
    >
      {children}
    </motion.div>
  )
);
FadeIn.displayName = "FadeIn";

// Slide up component
interface SlideUpProps {
  children: ReactNode;
  className?: string;
  delay?: number;
}

export const SlideUp = forwardRef<HTMLDivElement, SlideUpProps>(
  ({ children, className = "", delay = 0 }, ref) => (
    <motion.div
      ref={ref}
      initial={{ opacity: 0, y: 16 }}
      animate={{ opacity: 1, y: 0 }}
      transition={{ ...transitions.normal, delay }}
      className={className}
    >
      {children}
    </motion.div>
  )
);
SlideUp.displayName = "SlideUp";

// Scale in component
interface ScaleInProps {
  children: ReactNode;
  className?: string;
  delay?: number;
}

export const ScaleIn = forwardRef<HTMLDivElement, ScaleInProps>(
  ({ children, className = "", delay = 0 }, ref) => (
    <motion.div
      ref={ref}
      initial={{ opacity: 0, scale: 0.9 }}
      animate={{ opacity: 1, scale: 1 }}
      transition={{ ...transitions.springBounce, delay }}
      className={className}
    >
      {children}
    </motion.div>
  )
);
ScaleIn.displayName = "ScaleIn";

// Stagger list component
interface StaggerListProps {
  children: ReactNode;
  className?: string;
}

export const StaggerList = forwardRef<HTMLDivElement, StaggerListProps>(
  ({ children, className = "" }, ref) => (
    <motion.div
      ref={ref}
      initial="initial"
      animate="animate"
      variants={staggerContainerVariants}
      className={className}
    >
      {children}
    </motion.div>
  )
);
StaggerList.displayName = "StaggerList";

// Stagger item component
interface StaggerItemProps {
  children: ReactNode;
  className?: string;
}

export const StaggerItem = forwardRef<HTMLDivElement, StaggerItemProps>(
  ({ children, className = "" }, ref) => (
    <motion.div
      ref={ref}
      variants={staggerItemVariants}
      transition={transitions.normal}
      className={className}
    >
      {children}
    </motion.div>
  )
);
StaggerItem.displayName = "StaggerItem";

// Animated card component with hover effects
interface AnimatedCardProps extends ComponentProps<typeof motion.div> {
  children: ReactNode;
  className?: string;
  hoverScale?: number;
}

export const AnimatedCard = forwardRef<HTMLDivElement, AnimatedCardProps>(
  ({ children, className = "", hoverScale = 1.02, ...props }, ref) => (
    <motion.div
      ref={ref}
      whileHover={{ scale: hoverScale, y: -2 }}
      whileTap={{ scale: 0.98 }}
      transition={transitions.spring}
      className={className}
      {...props}
    >
      {children}
    </motion.div>
  )
);
AnimatedCard.displayName = "AnimatedCard";

// Animated button wrapper
interface AnimatedButtonProps {
  children: ReactNode;
  className?: string;
}

export const AnimatedButton = forwardRef<HTMLDivElement, AnimatedButtonProps>(
  ({ children, className = "" }, ref) => (
    <motion.div
      ref={ref}
      whileHover={{ scale: 1.02 }}
      whileTap={{ scale: 0.98 }}
      transition={transitions.spring}
      className={className}
    >
      {children}
    </motion.div>
  )
);
AnimatedButton.displayName = "AnimatedButton";

// Re-export motion for custom usage
export { motion, type Variants, type Transition };
