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

## 2. Non-browser clients

DSA10. The backend MAY accept `Authorization: Bearer <session-token>` for non-browser dashboard clients. This compatibility MUST NOT cause the dashboard browser to expose or persist the token.
