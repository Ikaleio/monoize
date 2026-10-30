import { useCallback, useState, useEffect, useId, useRef } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Eye, EyeOff } from "lucide-react";
import { MonoizeLogo } from "@/components/MonoizeLogo";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { FieldError } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { DashboardApiError } from "@/lib/api";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Skeleton } from "@/components/ui/skeleton";
import { CapCaptcha } from "@/components/CapCaptcha";
import {
  LanguageDropdown,
  ThemeDropdown,
} from "@/components/preference-menus";
import { useAuth } from "@/hooks/use-auth";
import { usePublicSettings } from "@/lib/swr";
import { motion, AnimatePresence } from "framer-motion";
import { AnimatedButton } from "@/components/ui/motion";

const authErrorKeys: Record<string, string> = {
  invalid_credentials: "auth.errors.invalidCredentials",
  invalid_username: "auth.usernameRequirements",
  reserved_username: "auth.errors.reservedUsername",
  invalid_password: "auth.errors.invalidPassword",
  registration_disabled: "auth.errors.registrationDisabled",
  username_exists: "auth.errors.usernameExists",
  account_disabled: "auth.errors.accountDisabled",
  captcha_required: "auth.errors.captchaRequired",
  captcha_invalid: "auth.captchaError",
  captcha_unavailable: "auth.captchaUnavailable",
};

const USERNAME_ERROR_CODES = new Set([
  "invalid_username",
  "reserved_username",
  "username_exists",
]);

const USERNAME_PATTERN = /^[a-zA-Z0-9_]{3,22}$/;
const MIN_PASSWORD_LENGTH = 8;

interface FieldErrors {
  username?: string;
  password?: string;
}

/** DSA9a: only an in-app path other than /login may be a return target. */
function returnPathFromState(state: unknown): string {
  const from = (
    state as { from?: { pathname?: string; search?: string; hash?: string } } | null
  )?.from;
  const pathname = from?.pathname;
  if (
    !pathname ||
    !pathname.startsWith("/") ||
    pathname.startsWith("//") ||
    pathname === "/login"
  ) {
    return "/dashboard";
  }
  return `${pathname}${from?.search ?? ""}${from?.hash ?? ""}`;
}

const containerVariants = {
  hidden: { opacity: 0 },
  visible: {
    opacity: 1,
    transition: {
      staggerChildren: 0.1,
      delayChildren: 0.2,
    },
  },
};

const itemVariants = {
  hidden: { opacity: 0, y: 20 },
  visible: { opacity: 1, y: 0 },
};

export function LoginPage() {
  const { t, i18n } = useTranslation();
  const [isLogin, setIsLogin] = useState(true);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [showPassword, setShowPassword] = useState(false);
  const [error, setError] = useState("");
  const [fieldErrors, setFieldErrors] = useState<FieldErrors>({});
  const [loading, setLoading] = useState(false);
  const [captchaToken, setCaptchaToken] = useState("");
  const [captchaHint, setCaptchaHint] = useState("");
  const [captchaResetKey, setCaptchaResetKey] = useState(0);
  const formId = useId();
  const usernameId = `${formId}-username`;
  const passwordId = `${formId}-password`;
  const usernameRef = useRef<HTMLInputElement>(null);
  const passwordRef = useRef<HTMLInputElement>(null);

  const {
    data: publicSettings,
    error: publicSettingsError,
    isLoading: publicSettingsLoading,
    isValidating: publicSettingsValidating,
    mutate: retryPublicSettings,
  } = usePublicSettings();
  const registrationEnabled = publicSettings?.registration_enabled ?? false;
  const siteDescription = publicSettings?.site_description?.trim() ?? "";
  const captchaActive = Boolean(
    publicSettings?.captcha_enabled && publicSettings.cap_api_endpoint,
  );

  const { login, register, user } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const returnPath = returnPathFromState(location.state);

  const handleCaptchaError = useCallback(() => {
    setError("auth.captchaError");
  }, []);

  const handleCaptchaToken = useCallback((token: string) => {
    setCaptchaToken(token);
    if (token) setCaptchaHint("");
  }, []);

  const resetCaptcha = useCallback(() => {
    setCaptchaToken("");
    setCaptchaResetKey((key) => key + 1);
  }, []);

  // Every successful sign-in lands here, so there is exactly one redirect path.
  useEffect(() => {
    if (user) {
      navigate(returnPath, { replace: true });
    }
  }, [user, navigate, returnPath]);

  const validate = (): FieldErrors => {
    const errors: FieldErrors = {};
    if (!username) {
      errors.username = "auth.errors.usernameRequired";
    } else if (!isLogin && !USERNAME_PATTERN.test(username)) {
      errors.username = "auth.usernameRequirements";
    }
    if (!password) {
      errors.password = "auth.errors.passwordRequired";
    } else if (!isLogin && password.length < MIN_PASSWORD_LENGTH) {
      errors.password = "auth.errors.invalidPassword";
    }
    return errors;
  };

  const showFieldErrors = (errors: FieldErrors) => {
    setFieldErrors(errors);
    if (errors.username) usernameRef.current?.focus();
    else if (errors.password) passwordRef.current?.focus();
  };

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError("");

    const errors = validate();
    showFieldErrors(errors);
    if (errors.username || errors.password) return;

    if (captchaActive && !captchaToken) {
      setCaptchaHint("auth.errors.captchaRequired");
      return;
    }

    setLoading(true);
    try {
      if (isLogin) {
        await login(username, password, captchaToken);
      } else {
        await register(username, password, captchaToken);
      }
    } catch (err) {
      const code = err instanceof DashboardApiError ? err.code : "";
      const key = Object.hasOwn(authErrorKeys, code)
        ? authErrorKeys[code]
        : "auth.errors.failed";
      if (USERNAME_ERROR_CODES.has(code)) {
        showFieldErrors({ username: key });
      } else if (code === "invalid_password") {
        showFieldErrors({ password: key });
      } else {
        setError(key);
      }
      resetCaptcha();
      if (captchaActive) setCaptchaHint("auth.captchaReverify");
    } finally {
      setLoading(false);
    }
  };

  const switchMode = (nextIsLogin: boolean) => {
    setIsLogin(nextIsLogin);
    setError("");
    setFieldErrors({});
    setCaptchaHint("");
    resetCaptcha();
  };

  return (
    <div className="h-dvh overflow-y-auto overscroll-contain p-4">
      <div className="flex min-h-full flex-col gap-4">
        <motion.div
          initial={{ opacity: 0, y: -20 }}
          animate={{ opacity: 1, y: 0 }}
          transition={{ duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
          className="flex shrink-0 justify-end gap-2"
        >
          <ThemeDropdown iconOnly />
          <LanguageDropdown iconOnly />
        </motion.div>

        <motion.div
          initial="hidden"
          animate="visible"
          variants={containerVariants}
          className="my-auto w-full max-w-md shrink-0 self-center"
        >
          <Card className="overflow-hidden">
            <CardHeader className="text-center">
              <motion.div
                variants={itemVariants}
                className="mx-auto mb-4 flex h-14 w-14 items-center justify-center rounded-2xl border bg-background p-2 text-foreground"
              >
                <MonoizeLogo className="h-full w-full" />
              </motion.div>
              <motion.div variants={itemVariants}>
                {publicSettings ? (
                  <CardTitle className="text-2xl">
                    <h1>{publicSettings.site_name}</h1>
                  </CardTitle>
                ) : publicSettingsError ? (
                  <CardTitle className="text-2xl">
                    <h1>Monoize</h1>
                  </CardTitle>
                ) : (
                  <Skeleton className="mx-auto h-8 w-48" />
                )}
              </motion.div>
              {publicSettings ? (
                siteDescription ? (
                  <motion.div variants={itemVariants}>
                    <CardDescription>{siteDescription}</CardDescription>
                  </motion.div>
                ) : null
              ) : publicSettingsError ? null : (
                <Skeleton className="mx-auto h-4 w-40" />
              )}
              <motion.div variants={itemVariants}>
                <CardDescription>
                  <h2>{isLogin ? t("auth.signInToAccount") : t("auth.createAccount")}</h2>
                </CardDescription>
              </motion.div>
            </CardHeader>
            <CardContent>
              <motion.form
                variants={containerVariants}
                onSubmit={handleSubmit}
                noValidate
                className="space-y-4"
              >
                <motion.div variants={itemVariants} className="space-y-2">
                  <Label htmlFor={usernameId}>{t("auth.username")}</Label>
                  <Input
                    ref={usernameRef}
                    id={usernameId}
                    name="username"
                    type="text"
                    autoComplete="username"
                    value={username}
                    onChange={(e) => {
                      setUsername(e.target.value);
                      setFieldErrors((current) => ({ ...current, username: undefined }));
                    }}
                    placeholder={t("auth.enterUsername")}
                    required
                    aria-invalid={fieldErrors.username ? true : undefined}
                    className="transition-all"
                  />
                  {!isLogin && (
                    <p className="text-sm text-muted-foreground">
                      {t("auth.usernameRequirements")}
                    </p>
                  )}
                  {fieldErrors.username && (
                    <FieldError>{t(fieldErrors.username)}</FieldError>
                  )}
                </motion.div>
                <motion.div variants={itemVariants} className="space-y-2">
                  <Label htmlFor={passwordId}>{t("auth.password")}</Label>
                  <div className="relative">
                    <Input
                      ref={passwordRef}
                      id={passwordId}
                      name="password"
                      type={showPassword ? "text" : "password"}
                      autoComplete={isLogin ? "current-password" : "new-password"}
                      value={password}
                      onChange={(e) => {
                        setPassword(e.target.value);
                        setFieldErrors((current) => ({ ...current, password: undefined }));
                      }}
                      placeholder={t("auth.enterPassword")}
                      required
                      aria-invalid={fieldErrors.password ? true : undefined}
                      className="pr-11 transition-all"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="icon"
                      className="absolute right-0 top-1/2 size-9 -translate-y-1/2 touch-manipulation"
                      aria-label={t(showPassword ? "auth.hidePassword" : "auth.showPassword")}
                      aria-pressed={showPassword}
                      onClick={() => setShowPassword((current) => !current)}
                    >
                      {showPassword ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
                    </Button>
                  </div>
                  {!isLogin && (
                    <p className="text-sm text-muted-foreground">
                      {t("auth.passwordRequirements")}
                    </p>
                  )}
                  {fieldErrors.password && (
                    <FieldError>{t(fieldErrors.password)}</FieldError>
                  )}
                </motion.div>
                <motion.div variants={itemVariants} className="space-y-2">
                  {publicSettingsError && !publicSettings ? (
                    <Alert variant="destructive">
                      <AlertDescription className="flex flex-wrap items-center justify-between gap-3">
                        <span className="min-w-0 flex-1 basis-40">
                          {t("auth.settingsLoadFailed")}
                        </span>
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          disabled={publicSettingsValidating}
                          onClick={() => void retryPublicSettings()}
                        >
                          {t(publicSettingsValidating ? "common.loading" : "common.retry")}
                        </Button>
                      </AlertDescription>
                    </Alert>
                  ) : publicSettingsLoading ? (
                    <Skeleton className="h-12 w-full rounded-md" />
                  ) : publicSettings?.captcha_enabled &&
                    !publicSettings.cap_api_endpoint ? (
                    <Alert variant="destructive">
                      <AlertDescription>
                        {t("auth.captchaUnavailable")}
                      </AlertDescription>
                    </Alert>
                  ) : publicSettings?.captcha_enabled &&
                    publicSettings.cap_api_endpoint ? (
                    <>
                      <CapCaptcha
                        key={i18n.language}
                        apiEndpoint={publicSettings.cap_api_endpoint}
                        language={i18n.language}
                        resetKey={captchaResetKey}
                        onTokenChange={handleCaptchaToken}
                        onError={handleCaptchaError}
                      />
                      {captchaHint && !captchaToken ? (
                        <p role="alert" className="text-sm text-destructive">
                          {t(captchaHint)}
                        </p>
                      ) : null}
                    </>
                  ) : null}
                </motion.div>
                <AnimatePresence mode="wait">
                  {error && (
                    <motion.div
                      initial={{ opacity: 0, y: -10, height: 0 }}
                      animate={{ opacity: 1, y: 0, height: "auto" }}
                      exit={{ opacity: 0, y: -10, height: 0 }}
                      transition={{ duration: 0.2 }}
                    >
                      <Alert variant="destructive">
                        <AlertDescription>{t(error)}</AlertDescription>
                      </Alert>
                    </motion.div>
                  )}
                </AnimatePresence>
                <motion.div variants={itemVariants}>
                  <AnimatedButton>
                    <Button
                      type="submit"
                      className="w-full"
                      disabled={
                        loading ||
                        publicSettingsLoading ||
                        !publicSettings ||
                        (publicSettings.captcha_enabled &&
                          !publicSettings.cap_api_endpoint)
                      }
                    >
                      {loading
                        ? t("common.loading")
                        : isLogin
                          ? t("auth.signIn")
                          : t("auth.signUp")}
                    </Button>
                  </AnimatedButton>
                </motion.div>
              </motion.form>
              {registrationEnabled && (
                <motion.div
                  initial={false}
                  animate="visible"
                  variants={itemVariants}
                  className="mt-4 text-center text-sm text-muted-foreground"
                >
                  <AnimatePresence mode="wait">
                    <motion.div
                      key={isLogin ? "login" : "register"}
                      initial={{ opacity: 0, y: 10 }}
                      animate={{ opacity: 1, y: 0 }}
                      exit={{ opacity: 0, y: -10 }}
                      transition={{ duration: 0.2 }}
                    >
                      {isLogin ? (
                        <>
                          {t("auth.noAccount")}{" "}
                          <button
                            type="button"
                            onClick={() => switchMode(false)}
                            className="text-primary underline-offset-4 hover:underline"
                          >
                            {t("auth.signUp")}
                          </button>
                        </>
                      ) : (
                        <>
                          {t("auth.hasAccount")}{" "}
                          <button
                            type="button"
                            onClick={() => switchMode(true)}
                            className="text-primary underline-offset-4 hover:underline"
                          >
                            {t("auth.signIn")}
                          </button>
                        </>
                      )}
                    </motion.div>
                  </AnimatePresence>
                </motion.div>
              )}
            </CardContent>
          </Card>
        </motion.div>
      </div>
    </div>
  );
}
