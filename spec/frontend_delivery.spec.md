# Frontend Delivery Specification

## 1. Scope

This spec defines how the bundled frontend is built, embedded, and served by Monoize.

## 2. Build and Packaging

FD-B1. Monoize MUST embed frontend build artifacts from `frontend/dist/` into the Rust binary.

FD-B2. Build pipeline MUST run frontend install/build before Rust compile when frontend sources change.

FD-B3. If frontend build fails, Rust build MUST fail.

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

## 6. HTTP Caching

FD-H1. Responses serving the embedded SPA entry document (`index.html`) for `GET /` and SPA fallback routes (for example `/dashboard/*`) MUST include `Cache-Control: no-store`.

FD-H2. Responses serving hashed static frontend assets under `frontend/dist/assets/` MUST include `Cache-Control: public, max-age=31536000, immutable`.
