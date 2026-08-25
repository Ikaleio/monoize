# npm CLI Distribution Specification

## 0. Status

- **Purpose:** Distribute the native Monoize server through Bun, npm, and pnpm without downloading binaries for unrelated platforms.
- **Scope:** Applies to `npm/`, the npm-package jobs in `.github/workflows/release.yml`, and the npm package named `monoize`.

## 1. Package set

NCD-P1. One release MUST publish exactly one root package version and one through six successful platform package versions under the npm package name `monoize`. A failed native build MUST omit only its matching platform version.

NCD-P2. The root package version MUST equal `[package].version` in `Cargo.toml`.

NCD-P3. The platform package versions and metadata MUST use this table:

| Rust target | Optional-dependency alias | Package version suffix | `os` | `cpu` | Executable |
| --- | --- | --- | --- | --- | --- |
| `x86_64-unknown-linux-musl` | `monoize-linux-x64` | `linux-x64` | `linux` | `x64` | `bin/monoize` |
| `aarch64-unknown-linux-musl` | `monoize-linux-arm64` | `linux-arm64` | `linux` | `arm64` | `bin/monoize` |
| `x86_64-apple-darwin` | `monoize-darwin-x64` | `darwin-x64` | `darwin` | `x64` | `bin/monoize` |
| `aarch64-apple-darwin` | `monoize-darwin-arm64` | `darwin-arm64` | `darwin` | `arm64` | `bin/monoize` |
| `x86_64-pc-windows-msvc` | `monoize-win32-x64` | `win32-x64` | `win32` | `x64` | `bin/monoize.exe` |
| `aarch64-pc-windows-msvc` | `monoize-win32-arm64` | `win32-arm64` | `win32` | `arm64` | `bin/monoize.exe` |

For Cargo version `<version>`, each platform package version MUST equal `<version>-<suffix>`.

NCD-P4. The root package MUST declare the six aliases in NCD-P3 as `optionalDependencies`. Each dependency value MUST use the npm alias form `npm:monoize@<platform-package-version>`.

NCD-P5. Each platform package MUST declare only its NCD-P3 `os` and `cpu` values. Bun, npm, and pnpm MUST therefore reject or omit that package on a non-matching operating system or CPU. A normal root-package installation MUST extract at most one native Monoize executable.

NCD-P6. The root package MUST expose `monoize` through `bin/monoize.js`. A platform package MUST NOT expose a package-manager binary link.

NCD-P7. The root package MUST require Node.js 18 or later. Its published JavaScript MUST also execute under Bun.

NCD-P8. Linux npm packages MUST contain statically linked musl executables. A Linux executable MUST NOT contain an ELF interpreter or a dynamic-library `DT_NEEDED` entry. It MUST run without a host GNU libc, musl libc, `libgcc`, or `libstdc++` shared library. The Linux packages therefore support both GNU-libc and musl-based Linux distributions on the matching CPU architecture.

## 2. Launcher behavior

NCD-L1. The launcher source MUST be TypeScript. The release workflow MUST use Bun to build it as Node-compatible ESM before npm packaging.

NCD-L2. The launcher MUST map `process.platform` and `process.arch` to exactly one row in NCD-P3. Any other pair MUST terminate before spawning a child and print the unsupported pair.

NCD-L3. The launcher MUST resolve the selected optional-dependency alias relative to the installed root package. It MUST execute only the binary contained by that dependency.

NCD-L4. The launcher MUST NOT use a lifecycle installation script. It MUST NOT download a binary at launcher runtime. Binary selection and download are package-manager operations governed by NCD-P4 and NCD-P5.

NCD-L5. If the selected optional dependency or its executable is missing, the launcher MUST exit nonzero. The error MUST identify the missing alias and show one reinstall command for the detected package manager. The supported commands are:

- `bun install -g monoize@latest`;
- `npm install -g monoize@latest`;
- `pnpm add -g monoize@latest`.

NCD-L6. The launcher MUST pass all user arguments to the native executable in their original order. It MUST inherit the parent working directory, environment, standard input, standard output, and standard error.

NCD-L7. On `SIGINT`, `SIGTERM`, or `SIGHUP`, the launcher MUST forward the same signal to the running native process and wait for it to exit.

NCD-L8. If the native process exits with a numeric status, the launcher MUST exit with that status. If the native process exits because of a signal, the launcher MUST terminate itself with the same signal.

## 3. Packaging and publication

NCD-R1. The npm staging command MUST reject a tag unless it equals `v` followed by the Cargo version.

NCD-R2. Each native build row in the release workflow MUST stage and pack the platform package that corresponds to that row. It MUST NOT place another target's binary in the package.

NCD-R3. One packaging job MUST build and pack the root package. It MUST verify that the package set contains the root tarball and a non-empty subset of the six platform tarballs for the release version. Every platform tarball in the set MUST match one row in NCD-P3.

NCD-R4. A manual release preflight MUST build, pack, and verify the npm package set. It MUST NOT publish an npm version.

NCD-R5. On a GitHub `release` publication, the npm publication job MUST publish every platform version present in the verified package set before the root version. An absent platform version MUST NOT block publication of the present versions.

NCD-R6. A platform package publication MUST use a non-`latest` npm dist-tag. Publishing a platform version MUST NOT change the root package's `latest` dist-tag.

NCD-R7. The root package publication MUST use the `latest` npm dist-tag. It MUST occur only after every platform publication present in the package set succeeds.

NCD-R8. npm publication MUST use npm Trusted Publishing through GitHub Actions OpenID Connect (OIDC). The publication job MUST run on a GitHub-hosted runner with `contents: read` and `id-token: write`. The job MUST NOT read an `NPM_TOKEN` Actions secret or write a registry authentication token.

NCD-R8a. The npm publication job MUST use Node.js `24.15.0` and npm CLI `12.0.2`. It MUST configure `https://registry.npmjs.org` as the registry before publication.

NCD-R9. The npm packaging job MUST run the TypeScript unit tests before it creates the root tarball. A test, build, pack, or package-set verification failure MUST prevent root-package publication.

NCD-R10. If the package set contains the packaging runner's platform tarball, the packaging job MUST serve the generated tarballs from an ephemeral local npm registry and install the root tarball with Bun, npm, and pnpm. Each installation MUST contain the root package, the matching platform alias, no non-matching platform alias, and the `monoize` binary link. Each client MUST download exactly one platform tarball. This verification MUST use installation scripts disabled. If that platform tarball is absent, the workflow MUST skip only this client-installation check; archive and manifest verification remain mandatory.

NCD-R11. A publication rerun MUST compare the local SHA-512 integrity with any existing npm version. If the bytes are identical and the required dist-tag already resolves to that version, publication MUST skip that version. Different bytes, a missing required dist-tag, or a required dist-tag that resolves to another version MUST fail publication. The workflow MUST NOT overwrite an existing version or run a separate dist-tag mutation command.

## 4. User commands

NCD-U1. Each of these one-shot commands MUST resolve the same root package and start the selected native executable:

```text
bunx monoize
npx monoize
pnpm dlx monoize
```

NCD-U2. Each global installation command MUST expose `monoize` on that package manager's configured global binary path. Running the exposed command MUST start the selected native executable.

```text
bun add --global monoize
npm install --global monoize
pnpm add --global monoize
```

NCD-U3. A project-local dependency installed with Bun, npm, or pnpm MUST expose the same `monoize` binary through that package manager's normal binary-link mechanism.
