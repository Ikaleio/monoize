import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { motion, useReducedMotion } from "framer-motion";
import { CreditCard, Pencil, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { springs } from "@/components/ui/motion";
import type { PaymentChannel } from "@/lib/api";
import {
  deletePaymentChannelOptimistic,
  updatePaymentChannelOptimistic,
  usePaymentChannels,
} from "@/lib/swr";
import { ChannelDialog } from "./channel-dialog";

/**
 * RC-M2 Channels tab: full §9.2 listing with optimistic enabled toggle,
 * create/edit dialogs, and a delete confirmation naming the channel.
 */
export function ChannelsTab() {
  const { t } = useTranslation();
  const reduced = useReducedMotion();
  const { data, isLoading } = usePaymentChannels();
  const channels = useMemo(() => data ?? [], [data]);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [editTarget, setEditTarget] = useState<PaymentChannel | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<PaymentChannel | null>(null);

  const toggleEnabled = async (channel: PaymentChannel, enabled: boolean) => {
    await updatePaymentChannelOptimistic(
      channel.id,
      { enabled },
      channels,
      (error) => toast.error(error.message),
    ).catch(() => undefined);
  };

  const handleDelete = async () => {
    if (!deleteTarget) return;
    try {
      await deletePaymentChannelOptimistic(deleteTarget.id, channels, (error) =>
        toast.error(error.message),
      );
      toast.success(t("common.success"));
    } catch {
      // optimistic helper already rolled back and toasted
    } finally {
      setDeleteTarget(null);
    }
  };

  return (
    <div className="space-y-4">
      <div className="flex justify-end">
        <Button
          onClick={() => {
            setEditTarget(null);
            setDialogOpen(true);
          }}
        >
          <Plus className="mr-2 h-4 w-4" />
          {t("payments.create")}
        </Button>
      </div>

      {isLoading ? (
        <div className="flex flex-col gap-3" aria-busy="true">
          {Array.from({ length: 3 }).map((_, index) => (
            <Skeleton key={index} className="h-12 w-full" />
          ))}
        </div>
      ) : channels.length === 0 ? (
        <EmptyState
          variant="card"
          icon={<CreditCard className="h-10 w-10 text-muted-foreground" />}
          title={t("payments.noChannelsTitle")}
          description={t("payments.noChannelsDescription")}
        />
      ) : (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          transition={{ duration: reduced ? 0 : 0.25 }}
          className="overflow-hidden rounded-lg border"
        >
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="bg-muted/50 text-left text-muted-foreground">
                <tr>
                  <th className="px-4 py-2.5 font-medium">{t("payments.name")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("payments.type")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("payments.currency")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("payments.usdRate")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("payments.creditBounds")}</th>
                  <th className="px-4 py-2.5 font-medium">{t("payments.enabled")}</th>
                  <th className="px-4 py-2.5" />
                </tr>
              </thead>
              <tbody>
                {channels.map((channel) => (
                  <motion.tr
                    key={channel.id}
                    layout={!reduced}
                    initial={reduced ? { opacity: 0 } : { opacity: 0, y: -8 }}
                    animate={{ opacity: 1, y: 0 }}
                    transition={reduced ? { duration: 0.15 } : springs.smooth}
                    className="border-t transition-colors hover:bg-accent/40"
                  >
                    <td className="px-4 py-3 font-medium">{channel.name}</td>
                    <td className="px-4 py-3 font-mono text-xs">{channel.type_id}</td>
                    <td className="px-4 py-3">{channel.currency}</td>
                    <td className="px-4 py-3 tabular-nums">{channel.usd_rate}</td>
                    <td className="px-4 py-3 tabular-nums text-muted-foreground">
                      ${channel.min_credit_usd} – ${channel.max_credit_usd}
                    </td>
                    <td className="px-4 py-3">
                      <Switch
                        checked={channel.enabled}
                        aria-label={t("payments.enabled")}
                        onCheckedChange={(checked) => toggleEnabled(channel, checked)}
                      />
                    </td>
                    <td className="px-4 py-3">
                      <div className="flex items-center justify-end gap-1">
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-9"
                          aria-label={t("common.edit")}
                          onClick={() => {
                            setEditTarget(channel);
                            setDialogOpen(true);
                          }}
                        >
                          <Pencil className="h-4 w-4" />
                        </Button>
                        <Button
                          variant="ghost"
                          size="icon"
                          className="size-9"
                          aria-label={t("common.delete")}
                          onClick={() => setDeleteTarget(channel)}
                        >
                          <Trash2 className="h-4 w-4 text-destructive" />
                        </Button>
                      </div>
                    </td>
                  </motion.tr>
                ))}
              </tbody>
            </table>
          </div>
        </motion.div>
      )}

      <ChannelDialog
        open={dialogOpen}
        onOpenChange={setDialogOpen}
        channel={editTarget}
        channels={channels}
      />

      <AlertDialog
        open={deleteTarget !== null}
        onOpenChange={(open) => !open && setDeleteTarget(null)}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("payments.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("payments.deleteDescription", { name: deleteTarget?.name })}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete}>
              {t("common.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
