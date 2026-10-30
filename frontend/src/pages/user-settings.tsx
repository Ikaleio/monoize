import { useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Save, User, Lock, Globe, Mail } from "lucide-react";
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
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import {
  LanguageDropdown,
  ThemeDropdown,
} from "@/components/preference-menus";
import { useAuth } from "@/hooks/use-auth";
import { PageWrapper, StaggerList, StaggerItem, motion, transitions } from "@/components/ui/motion";
import { PageHeader } from "@/components/ui/page-header";
import { apiErrorParam, toastApiError } from "@/lib/api-error";
import { updateMeOptimistic } from "@/lib/swr";
import { getGravatarUrl } from "@/lib/utils";
import { GroupsBadge } from "@/components/GroupsBadge";
import { toast } from "sonner";

const MIN_PASSWORD_LENGTH = 8;
const SAVED_FEEDBACK_MS = 2000;

/** Same rule as the backend `normalize_email` (dashboard-ui-layout.spec.md US6). */
function isValidEmail(email: string): boolean {
  if (email.length > 254 || /\s/.test(email)) return false;
  const at = email.indexOf("@");
  if (at <= 0 || email.indexOf("@", at + 1) !== -1) return false;
  const domain = email.slice(at + 1);
  return domain.includes(".") && domain.split(".").every(Boolean);
}

interface PasswordErrors {
  current?: string;
  next?: string;
  confirm?: string;
}

export function UserSettingsPage() {
  const { t } = useTranslation();
  const { user, changePassword } = useAuth();
  const formId = useId();
  const usernameId = `${formId}-username`;
  const emailId = `${formId}-email`;
  const currentPasswordId = `${formId}-current-password`;
  const newPasswordId = `${formId}-new-password`;
  const confirmPasswordId = `${formId}-confirm-password`;
  const emailRef = useRef<HTMLInputElement>(null);
  const currentPasswordRef = useRef<HTMLInputElement>(null);
  const newPasswordRef = useRef<HTMLInputElement>(null);
  const confirmPasswordRef = useRef<HTMLInputElement>(null);

  const [currentPassword, setCurrentPassword] = useState("");
  const [newPassword, setNewPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [passwordErrors, setPasswordErrors] = useState<PasswordErrors>({});
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [email, setEmail] = useState(user?.email || "");
  const [emailError, setEmailError] = useState("");
  const [savingEmail, setSavingEmail] = useState(false);
  const [savedEmail, setSavedEmail] = useState(false);

  const showPasswordErrors = (errors: PasswordErrors) => {
    setPasswordErrors(errors);
    if (errors.current) currentPasswordRef.current?.focus();
    else if (errors.next) newPasswordRef.current?.focus();
    else if (errors.confirm) confirmPasswordRef.current?.focus();
  };

  const handleSavePassword = async (event: React.FormEvent) => {
    event.preventDefault();
    const errors: PasswordErrors = {};
    if (!currentPassword) {
      errors.current = t("userSettings.errors.currentPasswordRequired");
    }
    if (newPassword.length < MIN_PASSWORD_LENGTH) {
      errors.next = t("userSettings.passwordTooShort");
    }
    if (newPassword !== confirmPassword) {
      errors.confirm = t("userSettings.passwordMismatch");
    }
    showPasswordErrors(errors);
    if (errors.current || errors.next || errors.confirm) return;

    setSaving(true);
    try {
      await changePassword(currentPassword, newPassword);
      setCurrentPassword("");
      setNewPassword("");
      setConfirmPassword("");
      setSaved(true);
      toast.success(t("userSettings.passwordChanged"));
      setTimeout(() => setSaved(false), SAVED_FEEDBACK_MS);
    } catch (error) {
      const param = apiErrorParam(error);
      if (param === "current_password") {
        showPasswordErrors({
          current: t("userSettings.errors.invalidCurrentPassword"),
        });
      } else if (param === "new_password") {
        showPasswordErrors({ next: t("auth.errors.invalidPassword") });
      } else {
        toastApiError(error, t);
      }
    } finally {
      setSaving(false);
    }
  };

  const handleSaveEmail = async (event: React.FormEvent) => {
    event.preventDefault();
    const emailValue = email.trim();
    if (emailValue && !isValidEmail(emailValue)) {
      setEmailError(t("userSettings.errors.invalidEmail"));
      emailRef.current?.focus();
      return;
    }
    setEmailError("");
    setSavingEmail(true);
    try {
      const updated = await updateMeOptimistic(
        { email: emailValue || null },
        user ?? undefined,
      );
      setEmail(updated.email ?? "");
      setSavedEmail(true);
      setTimeout(() => setSavedEmail(false), SAVED_FEEDBACK_MS);
    } catch (error) {
      if (apiErrorParam(error) === "email") {
        setEmailError(t("userSettings.errors.invalidEmail"));
        emailRef.current?.focus();
      } else {
        toastApiError(error, t);
      }
    } finally {
      setSavingEmail(false);
    }
  };

  const gravatarUrl = getGravatarUrl(email || user?.email, 96);

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
                  <Label htmlFor={usernameId}>{t("auth.username")}</Label>
                  <Input id={usernameId} value={user?.username || ""} disabled />
                  <p className="text-sm text-muted-foreground">
                    {t("userSettings.usernameCannotChange")}
                  </p>
                </div>
              </div>
              {user?.group_id && (
                <div className="space-y-2">
                  <p className="text-sm text-muted-foreground">{t("users.group")}</p>
                  <GroupsBadge groupIds={[user.group_id]} />
                </div>
              )}
              <Separator />
              <form onSubmit={handleSaveEmail} noValidate className="space-y-2">
                <Label htmlFor={emailId} className="inline-flex items-center gap-1">
                  <Mail className="size-4" />
                  {t("userSettings.email")}
                </Label>
                <Input
                  ref={emailRef}
                  id={emailId}
                  type="email"
                  autoComplete="email"
                  value={email}
                  onChange={(e) => {
                    setEmail(e.target.value);
                    setEmailError("");
                  }}
                  placeholder="you@example.com"
                  aria-invalid={emailError ? true : undefined}
                />
                <p className="text-sm text-muted-foreground">
                  {t("userSettings.emailDescription")}
                </p>
                {emailError && <FieldError>{emailError}</FieldError>}
                <Button type="submit" disabled={savingEmail} size="sm">
                  <Save data-icon="inline-start" aria-hidden="true" />
                  {savingEmail ? t("common.saving") : savedEmail ? t("common.saved") : t("common.save")}
                </Button>
              </form>
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
            <form onSubmit={handleSavePassword} noValidate>
              <CardContent>
                <FieldGroup>
                  <Field data-invalid={passwordErrors.current ? true : undefined}>
                    <FieldLabel htmlFor={currentPasswordId}>
                      {t("userSettings.currentPassword")}
                    </FieldLabel>
                    <Input
                      ref={currentPasswordRef}
                      id={currentPasswordId}
                      type="password"
                      autoComplete="current-password"
                      value={currentPassword}
                      onChange={(e) => {
                        setCurrentPassword(e.target.value);
                        setPasswordErrors((errors) => ({ ...errors, current: undefined }));
                      }}
                      aria-invalid={passwordErrors.current ? true : undefined}
                    />
                    {passwordErrors.current && <FieldError>{passwordErrors.current}</FieldError>}
                  </Field>
                  <Field data-invalid={passwordErrors.next ? true : undefined}>
                    <FieldLabel htmlFor={newPasswordId}>
                      {t("userSettings.newPassword")}
                    </FieldLabel>
                    <Input
                      ref={newPasswordRef}
                      id={newPasswordId}
                      type="password"
                      autoComplete="new-password"
                      value={newPassword}
                      onChange={(e) => {
                        setNewPassword(e.target.value);
                        setPasswordErrors((errors) => ({ ...errors, next: undefined }));
                      }}
                      aria-invalid={passwordErrors.next ? true : undefined}
                    />
                    <FieldDescription>
                      {t("userSettings.passwordRequirements")}
                    </FieldDescription>
                    {passwordErrors.next && <FieldError>{passwordErrors.next}</FieldError>}
                  </Field>
                  <Field data-invalid={passwordErrors.confirm ? true : undefined}>
                    <FieldLabel htmlFor={confirmPasswordId}>
                      {t("userSettings.confirmPassword")}
                    </FieldLabel>
                    <Input
                      ref={confirmPasswordRef}
                      id={confirmPasswordId}
                      type="password"
                      autoComplete="new-password"
                      value={confirmPassword}
                      onChange={(e) => {
                        setConfirmPassword(e.target.value);
                        setPasswordErrors((errors) => ({ ...errors, confirm: undefined }));
                      }}
                      aria-invalid={passwordErrors.confirm ? true : undefined}
                    />
                    {passwordErrors.confirm && <FieldError>{passwordErrors.confirm}</FieldError>}
                  </Field>
                </FieldGroup>
              </CardContent>
              <CardFooter>
                <Button type="submit" disabled={saving}>
                  <Save data-icon="inline-start" />
                  {saving
                    ? t("common.saving")
                    : saved
                      ? t("common.saved")
                      : t("userSettings.changePassword")}
                </Button>
              </CardFooter>
            </form>
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
                  <p className="text-sm font-medium leading-none">{t("userSettings.language")}</p>
                  <p className="text-sm text-muted-foreground">
                    {t("userSettings.languageDescription")}
                  </p>
                </div>
                <LanguageDropdown />
              </div>
              <Separator />
              <div className="flex items-center justify-between">
                <div className="space-y-0.5">
                  <p className="text-sm font-medium leading-none">{t("userSettings.theme")}</p>
                  <p className="text-sm text-muted-foreground">
                    {t("userSettings.themeDescription")}
                  </p>
                </div>
                <ThemeDropdown />
              </div>
            </CardContent>
          </Card>
        </StaggerItem>
      </StaggerList>
    </PageWrapper>
  );
}
