import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Save, User, Lock, Globe, Mail, Coins } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
} from "@/components/ui/field";
import { Separator } from "@/components/ui/separator";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { useAuth } from "@/hooks/use-auth";
import { useTheme } from "@/hooks/use-theme";
import { PageWrapper, StaggerList, StaggerItem, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { setLanguage, getCurrentLanguage } from "@/i18n";
import { updateMeOptimistic } from "@/lib/swr";
import { formatUsdDecimal } from "@/lib/exact-decimal";
import { getGravatarUrl } from "@/lib/utils";
import { GroupsBadge } from "@/components/GroupsBadge";
import { toast } from "sonner";

export function UserSettingsPage() {
  const { t } = useTranslation();
  const { user, changePassword, refreshUser } = useAuth();
  const { theme, setTheme } = useTheme();
  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [email, setEmail] = useState(user?.email || "");
  const [savingEmail, setSavingEmail] = useState(false);
  const [savedEmail, setSavedEmail] = useState(false);
  const currentLang = getCurrentLanguage();
  const passwordTooShort = newPassword.length > 0 && newPassword.length < 8;
  const passwordMismatch =
    confirmPassword.length > 0 && newPassword !== confirmPassword;

  const handleSavePassword = async () => {
    if (
      !currentPassword ||
      newPassword.length < 8 ||
      newPassword !== confirmPassword
    ) {
      return;
    }
    setSaving(true);
    try {
      await changePassword(currentPassword, newPassword);
      setCurrentPassword("");
      setNewPassword("");
      setConfirmPassword("");
      setSaved(true);
      toast.success(t("userSettings.passwordChanged"));
      setTimeout(() => setSaved(false), 2000);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("common.error"));
    } finally {
      setSaving(false);
    }
  };

  const handleSaveEmail = async () => {
    setSavingEmail(true);
    try {
      const emailValue = email.trim() || null;
      await updateMeOptimistic({ email: emailValue }, user ?? undefined);
      await refreshUser();
      setSavedEmail(true);
      setTimeout(() => setSavedEmail(false), 2000);
    } catch (error) {
      toast.error(error instanceof Error ? error.message : t("common.error"));
    } finally {
      setSavingEmail(false);
    }
  };

  const gravatarUrl = getGravatarUrl(email || user?.email, 96);

  const themeLabels = {
    light: t("theme.light"),
    dark: t("theme.dark"),
    system: t("theme.system"),
  };

  return (
    <PageWrapper className="space-y-6">
      <motion.div
        initial={{ opacity: 0, y: -10 }}
        animate={{ opacity: 1, y: 0 }}
        transition={transitions.normal}
      >
        <PageHeader title={t("userSettings.title")} description={t("userSettings.description")} />
      </motion.div>

      <StaggerList className="grid gap-6">
        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <User className="h-5 w-5" />
                {t("userSettings.profile")}
              </CardTitle>
              <CardDescription>{t("userSettings.profileDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex items-center gap-4">
                <Avatar className="h-16 w-16">
                  {gravatarUrl && <AvatarImage src={gravatarUrl} alt={user?.username} />}
                  <AvatarFallback className="text-lg">
                    {user?.username?.[0]?.toUpperCase() || "U"}
                  </AvatarFallback>
                </Avatar>
                <div className="flex-1 space-y-2">
                  <Label>{t("auth.username")}</Label>
                  <Input value={user?.username || ""} disabled />
                  <p className="text-sm text-muted-foreground">
                    {t("userSettings.usernameCannotChange")}
                  </p>
                </div>
              </div>
              <Separator />
              <div className="space-y-2">
                <Label htmlFor="email" className="inline-flex items-center gap-1">
                  <Mail className="size-4" />
                  {t("userSettings.email")}
                </Label>
                <Input
                  id="email"
                  type="email"
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  placeholder="you@example.com"
                />
                <p className="text-sm text-muted-foreground">
                  {t("userSettings.emailDescription")}
                </p>
                <Button
                  onClick={handleSaveEmail}
                  disabled={savingEmail}
                  size="sm"
                >
                  <Save className="mr-2 h-4 w-4" />
                  {savingEmail ? t("common.saving") : savedEmail ? t("common.saved") : t("common.save")}
                </Button>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <Coins className="h-5 w-5" />
                {t("userSettings.billing")}
              </CardTitle>
              <CardDescription>{t("userSettings.billingDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="grid gap-4 sm:grid-cols-2">
                <div className="space-y-1">
                  <p className="text-sm text-muted-foreground">{t("userSettings.currentBalance")}</p>
                  <p className="text-xl font-semibold tabular-nums">
                    {user?.balance_unlimited
                      ? t("users.unlimited")
                      : formatUsdDecimal(user?.balance_usd, 2)}
                  </p>
                </div>
              </div>
              {user?.group_id && (
                <div className="space-y-2">
                  <p className="text-sm text-muted-foreground">{t("users.group")}</p>
                  <GroupsBadge groupIds={[user.group_id]} />
                </div>
              )}
            </CardContent>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <Lock className="h-5 w-5" />
                {t("userSettings.security")}
              </CardTitle>
              <CardDescription>{t("userSettings.securityDescription")}</CardDescription>
            </CardHeader>
            <CardContent>
              <FieldGroup>
                <Field>
                  <FieldLabel htmlFor="current-password">
                    {t("userSettings.currentPassword")}
                  </FieldLabel>
                  <Input
                    id="current-password"
                    type="password"
                    autoComplete="current-password"
                    value={currentPassword}
                    onChange={(e) => setCurrentPassword(e.target.value)}
                  />
                </Field>
                <Field data-invalid={passwordTooShort || undefined}>
                  <FieldLabel htmlFor="new-password">
                    {t("userSettings.newPassword")}
                  </FieldLabel>
                  <Input
                    id="new-password"
                    type="password"
                    autoComplete="new-password"
                    value={newPassword}
                    onChange={(e) => setNewPassword(e.target.value)}
                    aria-invalid={passwordTooShort || undefined}
                  />
                  <FieldDescription>
                    {t("userSettings.passwordRequirements")}
                  </FieldDescription>
                  {passwordTooShort && (
                    <FieldError>{t("userSettings.passwordTooShort")}</FieldError>
                  )}
                </Field>
                <Field data-invalid={passwordMismatch || undefined}>
                  <FieldLabel htmlFor="confirm-password">
                    {t("userSettings.confirmPassword")}
                  </FieldLabel>
                  <Input
                    id="confirm-password"
                    type="password"
                    autoComplete="new-password"
                    value={confirmPassword}
                    onChange={(e) => setConfirmPassword(e.target.value)}
                    aria-invalid={passwordMismatch || undefined}
                  />
                  {passwordMismatch && (
                    <FieldError>{t("userSettings.passwordMismatch")}</FieldError>
                  )}
                </Field>
              </FieldGroup>
            </CardContent>
            <CardFooter>
              <Button
                onClick={handleSavePassword}
                disabled={
                  saving ||
                  !currentPassword ||
                  newPassword.length < 8 ||
                  newPassword !== confirmPassword
                }
              >
                <Save data-icon="inline-start" />
                {saving
                  ? t("common.saving")
                  : saved
                    ? t("common.saved")
                    : t("userSettings.changePassword")}
              </Button>
            </CardFooter>
          </Card>
        </StaggerItem>

        <StaggerItem>
          <Card>
            <CardHeader>
              <CardTitle className="flex items-center gap-2">
                <Globe className="h-5 w-5" />
                {t("userSettings.preferences")}
              </CardTitle>
              <CardDescription>{t("userSettings.preferencesDescription")}</CardDescription>
            </CardHeader>
            <CardContent className="space-y-4">
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <Label>{t("userSettings.language")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("userSettings.languageDescription")}
                  </p>
                </div>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="outline">{t(`language.${currentLang}`)}</Button>
                  </DropdownMenuTrigger>
                   <DropdownMenuContent align="end">
                    <DropdownMenuItem onClick={() => setLanguage("en")}>
                      {t("language.en")}
                    </DropdownMenuItem>
                    <DropdownMenuItem onClick={() => setLanguage("zh")}>
                      {t("language.zh")}
                    </DropdownMenuItem>
                    <DropdownMenuItem onClick={() => setLanguage("ja")}>
                      {t("language.ja")}
                    </DropdownMenuItem>
                    <DropdownMenuItem onClick={() => setLanguage("zh-TW")}>
                      {t("language.zh-TW")}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
              <Separator />
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <Label>{t("userSettings.theme")}</Label>
                  <p className="text-sm text-muted-foreground">
                    {t("userSettings.themeDescription")}
                  </p>
                </div>
                <DropdownMenu>
                  <DropdownMenuTrigger asChild>
                    <Button variant="outline">{themeLabels[theme]}</Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem onClick={() => setTheme("light")}>
                      {t("theme.light")}
                    </DropdownMenuItem>
                    <DropdownMenuItem onClick={() => setTheme("dark")}>
                      {t("theme.dark")}
                    </DropdownMenuItem>
                    <DropdownMenuItem onClick={() => setTheme("system")}>
                      {t("theme.system")}
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </CardContent>
          </Card>
        </StaggerItem>
      </StaggerList>
    </PageWrapper>
  );
}
