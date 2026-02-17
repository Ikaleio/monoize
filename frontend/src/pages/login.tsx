import { useState, useEffect } from "react";
import { useNavigate } from "react-router-dom";
import { useTranslation } from "react-i18next";
import { Layers3, Languages, Sun, Moon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useAuth } from "@/hooks/use-auth";
import { useTheme } from "@/hooks/use-theme";
import { usePublicSettings } from "@/lib/swr";
import { toggleLanguage } from "@/i18n";
import { motion, AnimatePresence } from "framer-motion";

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
  const { t } = useTranslation();
  const [isLogin, setIsLogin] = useState(true);
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);
  const { resolvedTheme, setTheme } = useTheme();

  const { data: publicSettings } = usePublicSettings();
  const registrationEnabled = publicSettings?.registration_enabled ?? true;
  const siteName = publicSettings?.site_name ?? "Monoize Dashboard";

  const { login, register, user } = useAuth();
  const navigate = useNavigate();

  useEffect(() => {
    if (user) {
      navigate("/dashboard");
    }
  }, [user, navigate]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    setError("");
    setLoading(true);

    try {
      if (isLogin) {
        await login(username, password);
      } else {
        await register(username, password);
      }
      navigate("/dashboard");
    } catch (err) {
      setError(err instanceof Error ? err.message : "An error occurred");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="flex min-h-screen flex-col items-center justify-center p-4">
      <motion.div
        initial={{ opacity: 0, y: -20 }}
        animate={{ opacity: 1, y: 0 }}
        transition={{ duration: 0.3, ease: [0.16, 1, 0.3, 1] }}
        className="absolute right-4 top-4 flex gap-2"
      >
        <motion.div whileHover={{ scale: 1.1 }} whileTap={{ scale: 0.9 }}>
          <Button
            variant="ghost"
            size="icon"
            onClick={() => setTheme(resolvedTheme === "dark" ? "light" : "dark")}
            title={t("theme.toggle")}
          >
            <AnimatePresence mode="wait">
              <motion.div
                key={resolvedTheme}
                initial={{ rotate: -90, opacity: 0 }}
                animate={{ rotate: 0, opacity: 1 }}
                exit={{ rotate: 90, opacity: 0 }}
                transition={{ duration: 0.15 }}
              >
                {resolvedTheme === "dark" ? (
                  <Moon className="h-5 w-5" />
                ) : (
                  <Sun className="h-5 w-5" />
                )}
              </motion.div>
            </AnimatePresence>
          </Button>
        </motion.div>
        <motion.div whileHover={{ scale: 1.1 }} whileTap={{ scale: 0.9 }}>
          <Button
            variant="ghost"
            size="icon"
            onClick={toggleLanguage}
            title={t("language.switchLanguage")}
          >
            <Languages className="h-5 w-5" />
          </Button>
        </motion.div>
      </motion.div>

      <motion.div
        initial="hidden"
        animate="visible"
        variants={containerVariants}
        className="w-full max-w-md"
      >
        <Card className="overflow-hidden">
          <CardHeader className="text-center">
            <motion.div
              variants={itemVariants}
              whileHover={{ scale: 1.05, rotate: 5 }}
              whileTap={{ scale: 0.95 }}
              transition={{ type: "spring", stiffness: 400, damping: 17 }}
              className="mx-auto mb-4 flex h-12 w-12 items-center justify-center rounded-2xl bg-primary text-primary-foreground"
            >
              <Layers3 className="h-6 w-6" />
            </motion.div>
            <motion.div variants={itemVariants}>
              <CardTitle className="text-2xl">{siteName}</CardTitle>
            </motion.div>
            <motion.div variants={itemVariants}>
              <CardDescription>
                {isLogin ? t("auth.signInToAccount") : t("auth.createAccount")}
              </CardDescription>
            </motion.div>
          </CardHeader>
          <CardContent>
            <motion.form
              variants={containerVariants}
              onSubmit={handleSubmit}
              className="space-y-4"
            >
              <motion.div variants={itemVariants} className="space-y-2">
                <Label htmlFor="username">{t("auth.username")}</Label>
                <Input
                  id="username"
                  type="text"
                  value={username}
                  onChange={(e) => setUsername(e.target.value)}
                  placeholder={t("auth.enterUsername")}
                  required
                  minLength={3}
                  maxLength={22}
                  pattern="[a-zA-Z0-9_]+"
                  title={t("auth.usernameRequirements")}
                  className="transition-all focus:scale-[1.01]"
                />
              </motion.div>
              <motion.div variants={itemVariants} className="space-y-2">
                <Label htmlFor="password">{t("auth.password")}</Label>
                <Input
                  id="password"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder={t("auth.enterPassword")}
                  required
                  minLength={8}
                  className="transition-all focus:scale-[1.01]"
                />
              </motion.div>
              <AnimatePresence mode="wait">
                {error && (
                  <motion.div
                    initial={{ opacity: 0, y: -10, height: 0 }}
                    animate={{ opacity: 1, y: 0, height: "auto" }}
                    exit={{ opacity: 0, y: -10, height: 0 }}
                    transition={{ duration: 0.2 }}
                    className="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive"
                  >
                    {error}
                  </motion.div>
                )}
              </AnimatePresence>
              <motion.div variants={itemVariants}>
                <motion.div
                  whileHover={{ scale: 1.02 }}
                  whileTap={{ scale: 0.98 }}
                >
                  <Button type="submit" className="w-full" disabled={loading}>
                    {loading ? t("common.loading") : isLogin ? t("auth.signIn") : t("auth.signUp")}
                  </Button>
                </motion.div>
              </motion.div>
            </motion.form>
            {registrationEnabled && (
              <motion.div
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
                          onClick={() => setIsLogin(false)}
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
                          onClick={() => setIsLogin(true)}
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
  );
}
