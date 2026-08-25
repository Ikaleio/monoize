export type SupportedPlatform = "linux" | "darwin" | "win32";
export type SupportedArchitecture = "x64" | "arm64";

export interface NativeTarget {
  readonly rustTarget: string;
  readonly packageAlias: string;
  readonly versionSuffix: string;
  readonly platform: SupportedPlatform;
  readonly architecture: SupportedArchitecture;
  readonly executable: "monoize" | "monoize.exe";
}

export const NATIVE_TARGETS: readonly NativeTarget[] = [
  {
    rustTarget: "x86_64-unknown-linux-musl",
    packageAlias: "monoize-linux-x64",
    versionSuffix: "linux-x64",
    platform: "linux",
    architecture: "x64",
    executable: "monoize",
  },
  {
    rustTarget: "aarch64-unknown-linux-musl",
    packageAlias: "monoize-linux-arm64",
    versionSuffix: "linux-arm64",
    platform: "linux",
    architecture: "arm64",
    executable: "monoize",
  },
  {
    rustTarget: "x86_64-apple-darwin",
    packageAlias: "monoize-darwin-x64",
    versionSuffix: "darwin-x64",
    platform: "darwin",
    architecture: "x64",
    executable: "monoize",
  },
  {
    rustTarget: "aarch64-apple-darwin",
    packageAlias: "monoize-darwin-arm64",
    versionSuffix: "darwin-arm64",
    platform: "darwin",
    architecture: "arm64",
    executable: "monoize",
  },
  {
    rustTarget: "x86_64-pc-windows-msvc",
    packageAlias: "monoize-win32-x64",
    versionSuffix: "win32-x64",
    platform: "win32",
    architecture: "x64",
    executable: "monoize.exe",
  },
  {
    rustTarget: "aarch64-pc-windows-msvc",
    packageAlias: "monoize-win32-arm64",
    versionSuffix: "win32-arm64",
    platform: "win32",
    architecture: "arm64",
    executable: "monoize.exe",
  },
] as const;

export function nativeTargetFor(
  platform: string,
  architecture: string,
): NativeTarget | undefined {
  return NATIVE_TARGETS.find(
    (target) => target.platform === platform && target.architecture === architecture,
  );
}

export function nativeTargetForRustTarget(rustTarget: string): NativeTarget | undefined {
  return NATIVE_TARGETS.find((target) => target.rustTarget === rustTarget);
}

export function platformVersion(version: string, target: NativeTarget): string {
  return `${version}-${target.versionSuffix}`;
}
