import { useTranslation } from "react-i18next";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";
import { StatusBadge } from "@/components/ui/status";
import { springs } from "@/components/ui/motion";
import { ORDER_STATUS_VARIANTS } from "@/lib/recharge";
import type { RechargeOrderStatus } from "@/lib/api";

/**
 * Recharge order status badge (RC-W4/RC-M3). Status transitions animate with
 * a spring scale-fade; `pending` breathes with a slow opacity pulse so a
 * polling row reads as live. Both effects collapse under reduced motion.
 */
export function OrderStatusBadge({ status }: { status: RechargeOrderStatus }) {
  const { t } = useTranslation();
  const reduced = useReducedMotion();

  return (
    <AnimatePresence mode="popLayout" initial={false}>
      <motion.span
        key={status}
        initial={reduced ? { opacity: 0 } : { opacity: 0, scale: 0.8 }}
        animate={
          status === "pending" && !reduced
            ? {
                opacity: [1, 0.55, 1],
                scale: 1,
                transition: {
                  opacity: { duration: 2, repeat: Infinity, ease: "easeInOut" },
                  scale: springs.snappy,
                },
              }
            : { opacity: 1, scale: 1 }
        }
        exit={reduced ? { opacity: 0 } : { opacity: 0, scale: 0.8 }}
        transition={reduced ? { duration: 0 } : springs.snappy}
        className="inline-flex"
      >
        <StatusBadge variant={ORDER_STATUS_VARIANTS[status]}>
          {t(`wallet.status.${status}`)}
        </StatusBadge>
      </motion.span>
    </AnimatePresence>
  );
}
