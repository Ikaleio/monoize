import { useState, type ReactNode } from "react";
import { Link } from "react-router-dom";
import { Check, Copy } from "lucide-react";
import { useTranslation } from "react-i18next";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { useCopyToClipboard } from "@/hooks/use-copy-to-clipboard";
import type { PublicSystemSettings, User } from "@/lib/api";
import { cn } from "@/lib/utils";
import { resolveApiBase } from "./utils";

const API_BASE_COPIED_KEY = "monoize-home-api-base-copied";

function readCopied(): boolean {
  try {
    return window.localStorage.getItem(API_BASE_COPIED_KEY) === "1";
  } catch {
    return false;
  }
}

function Step({
  number,
  done,
  title,
  action,
}: {
  number: number;
  done: boolean;
  title: string;
  action: ReactNode;
}) {
  return (
    <li className="flex min-w-0 flex-col gap-3 p-4">
      <div className="flex min-w-0 items-center gap-2">
        <span
          className={cn(
            "flex size-6 shrink-0 items-center justify-center rounded-full border text-xs font-medium tabular-nums",
            done && "border-success text-success",
          )}
        >
          {done ? <Check className="size-3.5" aria-hidden="true" /> : number}
        </span>
        <span className="min-w-0 text-sm font-medium">{title}</span>
      </div>
      <div>{action}</div>
    </li>
  );
}

interface GettingStartedCardProps {
  user: User;
  settings: PublicSystemSettings | undefined;
  hasApiKey: boolean;
  canTopUp: boolean;
}

/** DH-4a: shown until the account has an API key and at least one request. */
export function GettingStartedCard({
  user,
  settings,
  hasApiKey,
  canTopUp,
}: GettingStartedCardProps) {
  const { t } = useTranslation();
  const { copied, copy } = useCopyToClipboard();
  const [copiedOnce, setCopiedOnce] = useState(readCopied);

  const copyApiBase = async () => {
    await copy(resolveApiBase(settings).url);
    try {
      window.localStorage.setItem(API_BASE_COPIED_KEY, "1");
    } catch {
      // Storage may be unavailable; the check mark still applies to this mount.
    }
    setCopiedOnce(true);
  };

  const hasBalance = /^[1-9]\d*$/.test(user.balance_nano_usd ?? "");

  return (
    <section>
      <Card>
        <CardHeader className="p-4 pb-0">
          <CardTitle className="text-base font-semibold leading-none tracking-tight">
            {t("dashboard.gettingStarted.title")}
          </CardTitle>
        </CardHeader>
        <CardContent className="p-0">
          <ol
            className={cn(
              "grid grid-cols-1 divide-y sm:divide-x sm:divide-y-0",
              canTopUp ? "sm:grid-cols-3" : "sm:grid-cols-2",
            )}
          >
            <Step
              number={1}
              done={hasApiKey}
              title={t("dashboard.gettingStarted.createKey")}
              action={
                <Button asChild variant="outline" size="sm">
                  <Link to="/dashboard/tokens">
                    {t("dashboard.gettingStarted.createKeyAction")}
                  </Link>
                </Button>
              }
            />
            <Step
              number={2}
              done={copiedOnce}
              title={t("dashboard.gettingStarted.copyBase")}
              action={
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  disabled={settings === undefined}
                  onClick={() => void copyApiBase()}
                >
                  {copied ? (
                    <Check data-icon="inline-start" aria-hidden="true" />
                  ) : (
                    <Copy data-icon="inline-start" aria-hidden="true" />
                  )}
                  {t("dashboard.gettingStarted.copyBaseAction")}
                </Button>
              }
            />
            {canTopUp ? (
              <Step
                number={3}
                done={hasBalance}
                title={t("dashboard.gettingStarted.topUp")}
                action={
                  <Button asChild variant="outline" size="sm">
                    <Link to="/dashboard/wallet?tab=recharge">
                      {t("dashboard.gettingStarted.topUpAction")}
                    </Link>
                  </Button>
                }
              />
            ) : null}
          </ol>
        </CardContent>
      </Card>
    </section>
  );
}
