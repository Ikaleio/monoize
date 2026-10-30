# Dashboard Session Authentication Specification

## 0. Scope

This specification defines browser storage and transport of dashboard sessions.

## 1. Session cookie

DSA1. Successful login and registration MUST set `monoize_session` with attributes `HttpOnly`, `Secure`, `SameSite=Strict`, and `Path=/`.

DSA2. Dashboard browser requests MUST send cookies with `credentials: "include"`.

DSA3. The dashboard browser MUST NOT store the dashboard session token in `localStorage`, `sessionStorage`, or IndexedDB.

DSA4. The dashboard browser MUST NOT read the dashboard session token from browser storage.

DSA5. The dashboard browser MUST NOT add an `Authorization` header for dashboard session authentication. This rule applies to REST and SSE requests.

DSA6. The dashboard browser MUST determine its authenticated state by calling `GET /api/dashboard/auth/me` with the session cookie.

DSA7. Logout MUST invalidate the server session identified by the cookie and MUST expire the `monoize_session` cookie.

DSA8. If a dashboard endpoint requires authentication and the request contains neither a `monoize_session` cookie nor a Bearer session token, the backend MUST return HTTP `401` with code `unauthorized` and message `missing dashboard session`.

DSA9. A dashboard API response with HTTP `401` and error code `unauthorized` MUST invalidate the browser's authenticated state. The dashboard MUST clear cached authenticated data and navigate to `/login` instead of rendering the response error in the current page.

DSA9a. When the dashboard shell redirects an unauthenticated visitor to `/login`, it MUST pass the current location as router state `from` and MUST replace the history entry. After successful login or registration, and when an already authenticated user opens `/login`, the page MUST navigate with `replace` to `from.pathname + from.search + from.hash`. If `from` is absent, its pathname does not start with exactly one `/`, or its pathname is `/login`, the target MUST be `/dashboard`.

DSA9b. The dashboard browser MUST hold the session user in SWR key `SWR_KEYS.ME`. While a session is active, it MUST revalidate that key on window focus and every 60000 ms. A failed revalidation other than DSA9 MUST retain the last user object. After DSA9 or logout, it MUST stop requesting `GET /api/dashboard/auth/me` until the next login or registration.

## 2. Non-browser clients

DSA10. The backend MAY accept `Authorization: Bearer <session-token>` for non-browser dashboard clients. This compatibility MUST NOT cause the dashboard browser to expose or persist the token.
