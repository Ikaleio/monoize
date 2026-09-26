import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { CreditCard, Pencil, Plus, Trash2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Skeleton } from "@/components/ui/skeleton";
import { EmptyState } from "@/components/ui/empty-state";
import { DataTableShell } from "@/components/ui/data-table-shell";
import {
  DataList,
  DataListActions,
  DataListBody,
  DataListCell,
  DataListHead,
  DataListHeader,
  DataListRow,
} from "@/components/ui/data-list";
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
import type { PaymentChannel } from "@/lib/api";
import { cn } from "@/lib/utils";
import {
  deletePaymentChannelOptimistic,
  updatePaymentChannelOptimistic,
  usePaymentChannels,
} from "@/lib/swr";

interface ChannelsTabProps {
  onCreate: () => void;
  onEdit: (channel: PaymentChannel) => void;
}

/**
 * RC-M2 Channels tab: full §9.2 listing with optimistic enabled toggle and a
 * delete confirmation naming the channel. The page owns the create/edit dialog.
 */
export function ChannelsTab({ onCreate, onEdit }: ChannelsTabProps) {
  const { t } = useTranslation();
  const { data, isLoading } = usePaymentChannels();
  const channels = useMemo(() => data ?? [], [data]);
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

  if (isLoading) {
    return (
      <DataTableShell aria-busy="true">
        <div className="divide-y">
          {Array.from({ length: 3 }).map((_, index) => (
            <div key={index} className="px-4 py-3">
              <Skeleton className="h-6 w-full" />
            </div>
          ))}
        </div>
      </DataTableShell>
    );
  }

  return (
    <>
      <DataTableShell
        isEmpty={channels.length === 0}
        emptyState={
          <EmptyState
            icon={<CreditCard className="size-10" aria-hidden="true" />}
            title={t("payments.noChannelsTitle")}
            description={t("payments.noChannelsDescription")}
            action={
              <Button onClick={onCreate}>
                <Plus aria-hidden="true" />
                {t("payments.create")}
              </Button>
            }
          />
        }
      >
        <DataList columns="minmax(0,1.2fr) 5rem 4.5rem minmax(0,1fr) minmax(0,1fr) 4.5rem 5rem">
          <DataListHeader>
            <DataListHead>{t("payments.name")}</DataListHead>
            <DataListHead>{t("payments.type")}</DataListHead>
            <DataListHead>{t("payments.currency")}</DataListHead>
            <DataListHead align="end">{t("payments.rate")}</DataListHead>
            <DataListHead align="end">{t("payments.creditBounds")}</DataListHead>
            <DataListHead>{t("payments.enabled")}</DataListHead>
            <DataListHead align="end">{t("common.actions")}</DataListHead>
          </DataListHeader>
          <DataListBody aria-label={t("payments.channelsTab")}>
            {channels.map((channel) => (
              <DataListRow key={channel.id}>
                <DataListCell primary>
                  <span
                    className={cn("block truncate font-medium", !channel.enabled && "text-muted-foreground")}
                    title={channel.name}
                  >
                    {channel.name}
                  </span>
                </DataListCell>
                <DataListCell label={t("payments.type")}>
                  <span className="font-mono">{channel.type_id}</span>
                </DataListCell>
                <DataListCell label={t("payments.currency")}>{channel.currency}</DataListCell>
                <DataListCell label={t("payments.rate")} align="end">
                  <span className="tabular-nums">
                    {t("payments.rateValue", { rate: channel.usd_rate, currency: channel.currency })}
                  </span>
                </DataListCell>
                <DataListCell label={t("payments.creditBounds")} align="end">
                  <span className="tabular-nums">
                    ${channel.min_credit_usd} – ${channel.max_credit_usd}
                  </span>
                </DataListCell>
                <DataListCell label={t("payments.enabled")}>
                  <Switch
                    className="align-middle"
                    checked={channel.enabled}
                    aria-label={t("common.enableItem", { name: channel.name })}
                    onCheckedChange={(checked) => toggleEnabled(channel, checked)}
                  />
                </DataListCell>
                <DataListActions>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    aria-label={t("common.editItem", { name: channel.name })}
                    onClick={() => onEdit(channel)}
                  >
                    <Pencil />
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-11 touch-manipulation sm:size-9"
                    aria-label={t("common.deleteItem", { name: channel.name })}
                    onClick={() => setDeleteTarget(channel)}
                  >
                    <Trash2 className="text-error-foreground" />
                  </Button>
                </DataListActions>
              </DataListRow>
            ))}
          </DataListBody>
        </DataList>
      </DataTableShell>

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
    </>
  );
}
