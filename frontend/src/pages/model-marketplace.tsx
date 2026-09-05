import { QueryError } from "@/components/ui/query-error";
import { Button } from "@/components/ui/button";
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { Store } from "lucide-react";
import { ModelMarketplaceCard } from "@/components/model-marketplace/model-card";
import { ModelMarketplaceSkeleton } from "@/components/model-marketplace/marketplace-skeleton";
import { Badge } from "@/components/ui/badge";
import { Field, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { useMarketplaceModels } from "@/lib/swr";
import { PageWrapper, motion, transitions } from "@/components/ui/motion";
import { EmptyState } from "@/components/ui/empty-state";
import { PageHeader } from "@/components/ui/page-header";

export function ModelMarketplacePage() {
  const { t } = useTranslation();
  const { data, error, isLoading, isValidating, mutate } =
    useMarketplaceModels();
  const records = useMemo(() => data ?? [], [data]);
  const [search, setSearch] = useState("");

  const filtered = useMemo(() => {
    const query = search.trim().toLocaleLowerCase();
    if (!query) return records;
    return records.filter((record) =>
      record.model_id.toLocaleLowerCase().includes(query),
    );
  }, [records, search]);

  if (isLoading && !error && data === undefined) {
    return (
      <PageWrapper>
        <ModelMarketplaceSkeleton />
      </PageWrapper>
    );
  }

  return (
    <PageWrapper className="flex flex-col gap-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader
          title={t("modelMarketplace.title")}
          description={t("modelMarketplace.description")}
          className="[&_h1]:font-sans"
          actions={
            data !== undefined ? (
              <Badge variant="outline" className="text-sm font-normal">
                {t("modelMarketplace.resultCount", {
                  filtered: filtered.length,
                  total: records.length,
                })}
              </Badge>
            ) : undefined
          }
        />
      </motion.div>

      {error && (
        <QueryError
          onRetry={mutate}
          retrying={isValidating}
          stale={data !== undefined}
        />
      )}
      {data !== undefined && (
        <Field className="max-w-xl">
          <FieldLabel htmlFor="model-marketplace-search" className="sr-only">
            {t("modelMarketplace.searchPlaceholder")}
          </FieldLabel>
          <Input
            id="model-marketplace-search"
            type="search"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            placeholder={t("modelMarketplace.searchPlaceholder")}
            autoComplete="off"
          />
        </Field>
      )}

      <motion.div
        initial={{ opacity: 0, y: 20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ delay: 0.1, ...transitions.normal }}
      >
        {data === undefined ? null : filtered.length === 0 ? (
          <EmptyState
            icon={<Store className="size-12" />}
            title={t(
              records.length > 0
                ? "modelMarketplace.noMatches"
                : "modelMarketplace.noModels",
            )}
            description={t(
              records.length > 0
                ? "modelMarketplace.noMatchesDesc"
                : "modelMarketplace.noModelsDesc",
            )}
            action={
              records.length > 0 ? (
                <Button variant="outline" onClick={() => setSearch("")}>
                  {t("modelMarketplace.clearSearch")}
                </Button>
              ) : undefined
            }
          />
        ) : (
          <section
            aria-label={t("modelMarketplace.title")}
            className="grid grid-cols-1 gap-4 md:grid-cols-2 lg:grid-cols-12"
          >
            {filtered.map((record, index) => (
              <ModelMarketplaceCard
                key={record.model_id}
                index={index}
                record={record}
              />
            ))}
          </section>
        )}
      </motion.div>
    </PageWrapper>
  );
}
