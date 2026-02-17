import { useState } from "react";
import { useTranslation } from "react-i18next";
import { Save, User, Lock, Globe } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Separator } from "@/components/ui/separator";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { useAuth } from "@/hooks/use-auth";
import { useTheme } from "@/hooks/use-theme";
import { PageWrapper, StaggerList, StaggerItem, motion, transitions } from "@/components/ui/motion";
import { setLanguage, getCurrentLanguage } from "@/i18n";

export function UserSettingsPage() {
  const { t } = useTranslation();
  const { user } = useAuth();
  const { theme, setTheme } = useTheme();
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const currentLang = getCurrentLanguage();

  const handleSavePassword = async () => {
    if (!password || password !== confirmPassword) return;
    setSaving(true);
    // TODO: Implement password change API
    setTimeout(() => {
      setSaving(false);
      setSaved(true);
      setPassword("");
      setConfirmPassword("");
      setTimeout(() => setSaved(false), 2000);
    }, 1000);
  };

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
        <h1 className="text-3xl font-bold tracking-tight">{t("userSettings.title")}</h1>
        <p className="text-muted-foreground">{t("userSettings.description")}</p>
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
              <div className="space-y-2">
                <Label>{t("auth.username")}</Label>
                <Input value={user?.username || ""} disabled />
                <p className="text-sm text-muted-foreground">
                  {t("userSettings.usernameCannotChange")}
                </p>
              </div>
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
            <CardContent className="space-y-4">
              <div className="space-y-2">
                <Label htmlFor="new-password">{t("userSettings.newPassword")}</Label>
                <Input
                  id="new-password"
                  type="password"
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  placeholder="••••••••"
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="confirm-password">{t("userSettings.confirmPassword")}</Label>
                <Input
                  id="confirm-password"
                  type="password"
                  value={confirmPassword}
                  onChange={(e) => setConfirmPassword(e.target.value)}
                  placeholder="••••••••"
                />
              </div>
              <Button
                onClick={handleSavePassword}
                disabled={saving || !password || password !== confirmPassword}
              >
                <Save className="mr-2 h-4 w-4" />
                {saving ? t("common.saving") : saved ? t("common.saved") : t("userSettings.changePassword")}
              </Button>
            </CardContent>
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
