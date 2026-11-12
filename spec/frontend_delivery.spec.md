# Frontend Delivery Specification

## 1. Scope

This spec defines how the bundled frontend is built, embedded, and served by Monoize.

## 2. Build and Packaging

FD-B1. Monoize MUST embed frontend build artifacts from `frontend/dist/` into the Rust binary.

FD-B2. Build pipeline MUST run frontend install/build before Rust compile when frontend sources change.

FD-B3. If frontend build fails, Rust build MUST fail.

FD-B4. A production `bun run build` MUST emit an entry JavaScript chunk of at most
900,000 bytes before compression. Every emitted JavaScript chunk MUST remain at
or below 1,000,000 bytes before compression. Measure file sizes in
`frontend/dist/assets/`, not gzip transfer sizes.

FD-B5. Each route page MUST use a dynamic import. Loading `/login` MUST NOT fetch
dashboard page modules, Recharts, CodeMirror, the custom-transform formatter, or
the AI SDK. The dashboard shell MAY load eagerly. A page module MUST load only
when its route renders; revisiting a loaded route MAY reuse the browser module
cache.

FD-B6. Each pending route import MUST render a route-level skeleton. Dashboard
skeletons MUST remain inside the existing navigation shell and `PageWrapper`.
A dashboard route skeleton MUST be one of the shared page skeletons (table page,
card grid, settings page, or page header only) or empty; a page whose own
loading state is one of these MUST use the same one. The login skeleton MUST
retain the centered authentication card. The loading region MUST expose a localized
loading status and `aria-busy=true`.

FD-B7. Route splitting MUST preserve paths, title `handle` i18n keys, navigation,
auth return targets, and admin-only guards. An unauthorized page MUST NOT mount
or fetch its page module. A rejected page import MUST reach the existing route
error boundary without removing the dashboard shell.

## 3. Runtime Routing

FD-R1. `GET /` MUST return embedded `index.html`.

FD-R2. `GET /<path>` MUST return embedded asset if `<path>` exists under `frontend/dist/`.

FD-R3. Unknown UI paths MUST return embedded `index.html` (SPA fallback).

FD-R4. Non-GET unknown API paths MUST return `404`.

FD-R5. Dashboard SPA routes under `/dashboard/*` (for example `/dashboard/providers`, `/dashboard/users`, `/dashboard/tokens`, `/dashboard/models`) MUST resolve to embedded `index.html` on direct browser navigation and hard refresh.

FD-R6. Dashboard API handlers MUST be exposed only under `/api/dashboard/*` and MUST NOT intercept direct browser `GET` requests for `/dashboard/*` SPA paths.

## 4. Frontend API Base

FD-A1. Frontend MUST use `/api` as backend base path in development and production.

FD-A2. Vite dev server MUST proxy `/api/*` to backend in development.

## 5. CSP Compatibility

FD-C1. The embedded frontend entry document (`frontend/dist/index.html`) MUST remain compatible with the backend Content Security Policy that sets `script-src 'self'`.

FD-C2. The entry document MUST NOT contain inline `<script>` blocks or inline event-handler attributes. Any startup logic required before React mounts (for example theme resolution) MUST be delivered through same-origin external script modules.

FD-C3. The backend MUST replace the entry document's CSP nonce placeholder with the request's fresh nonce before returning `index.html`. Hashed static assets MUST be returned without content substitution.

FD-C4. The backend MUST set `script-src 'self' 'wasm-unsafe-eval' 'nonce-<request-nonce>'`. The nonce MUST differ between independent requests. The frontend MAY read the nonce from a metadata element and pass it to a library that creates inline scripts, but the frontend MUST NOT create its own executable inline script in `index.html`. The policy MUST NOT include `'unsafe-eval'`.

## 6. HTTP Caching

FD-H1. Responses serving the embedded SPA entry document (`index.html`) for `GET /` and SPA fallback routes (for example `/dashboard/*`) MUST include `Cache-Control: no-store`.

FD-H2. Responses serving hashed static frontend assets under `frontend/dist/assets/` MUST include `Cache-Control: public, max-age=31536000, immutable`.
