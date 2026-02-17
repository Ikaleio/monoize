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

## 4. Frontend API Base

FD-A1. Frontend MUST use `/api` as backend base path in development and production.

FD-A2. Vite dev server MUST proxy `/api/*` to backend in development.

