import { useTranslation } from "react-i18next";
import { Link } from "react-router-dom";
import { Link2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { Skeleton } from "@/components/ui/skeleton";
import type { CustomTransformReference } from "@/lib/api";

/** Chain editor entry points (custom-js-transforms.spec.md CJS-UI-7). */
export function ChainEntryLinks() {
  const { t } = useTranslation();
  const linkClass = "underline underline-offset-4 hover:text-primary";
  return (
    <>
      {t("customTransforms.attachHint")}{" "}
      <Link to="/dashboard/providers" className={linkClass}>
        {t("customTransforms.linkProviders")}
      </Link>
      {" · "}
      <Link to="/dashboard/admin-settings?setting=global_transforms" className={linkClass}>
        {t("customTransforms.globalChain")}
      </Link>
      {" · "}
      <Link to="/dashboard/tokens" className={linkClass}>
        {t("customTransforms.linkApiKeys")}
      </Link>
    </>
  );
}

function referenceHref(reference: CustomTransformReference, currentUserId: string | undefined) {
  switch (reference.kind) {
    case "provider":
      return `/dashboard/providers?provider=${encodeURIComponent(reference.id)}`;
    case "global":
      return "/dashboard/admin-settings?setting=global_transforms";
    case "api_key":
      // The API key page only edits the admin's own keys; other keys are reached via the owner.
      return reference.owner_user_id === currentUserId
        ? "/dashboard/tokens"
        : `/dashboard/users?q=${encodeURIComponent(reference.owner_username ?? "")}`;
  }
}

/** Card footer reference count with a popover of referencing chains (CJS-UI-8). */
export function TransformReferences({
  references,
  loading,
  currentUserId,
}: {
  /** Undefined when the references request failed. */
  references: CustomTransformReference[] | undefined;
  loading: boolean;
  currentUserId: string | undefined;
}) {
  const { t } = useTranslation();

  if (loading) return <Skeleton className="h-5 w-24" />;
  if (!references) return null;
  if (references.length === 0) {
    return <span className="text-sm text-muted-foreground">{t("customTransforms.notUsed")}</span>;
  }

  const groups = [
    { kind: "provider", label: t("customTransforms.referencesProviders") },
    { kind: "global", label: t("customTransforms.referencesGlobal") },
    { kind: "api_key", label: t("customTransforms.referencesApiKeys") },
  ] as const;

  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button variant="ghost" size="sm" className="-ml-3 text-muted-foreground">
          <Link2 data-icon="inline-start" aria-hidden="true" />
          {t("customTransforms.usedIn", { count: references.length })}
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-0">
        <p className="flex h-11 items-center border-b px-3 text-sm">
          {t("customTransforms.referencesTitle")}
        </p>
        <div className="max-h-[300px] overflow-y-auto overflow-x-hidden">
          {groups.map((group) => {
            const items = references.filter((reference) => reference.kind === group.kind);
            if (items.length === 0) return null;
            return (
              <div key={group.kind} className="overflow-hidden p-1 text-foreground">
                <p className="px-2 py-1.5 text-xs font-medium text-muted-foreground">{group.label}</p>
                <ul>
                  {items.map((reference) => (
                    <li key={`${reference.kind}:${reference.id}`}>
                      <Link
                        to={referenceHref(reference, currentUserId)}
                        className="relative flex select-none items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-none hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground"
                      >
                        <span className="min-w-0 truncate">
                          {reference.kind === "global"
                            ? t("customTransforms.globalChain")
                            : reference.kind === "api_key" &&
                                reference.owner_user_id !== currentUserId
                              ? t("customTransforms.apiKeyOwner", {
                                  name: reference.name,
                                  username: reference.owner_username,
                                })
                              : reference.name}
                        </span>
                      </Link>
                    </li>
                  ))}
                </ul>
              </div>
            );
          })}
        </div>
      </PopoverContent>
    </Popover>
  );
}
