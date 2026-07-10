# Cardivex TdP Screening API Reference

Welcome to the frontend API reference! This backend powers the Cardivex TdP cardiotoxicity screening app, providing secure authentication, role-based access control, an admin panel, and an AI prediction proxy communicating with a Gradio model.

**Base URLs**:
- **Local Development**: `http://localhost:3000`
- **Production**: `https://cardiotox-backend.onrender.com` *(Note: This exact URL string is the genuine deployed domain)*

---

> [!IMPORTANT] 
> **TL;DR / Golden Rules**
> - **No Tokens**: We use secure, HTTP-only cookies. You MUST send `credentials: 'include'` (fetch) or `withCredentials: true` (axios) with **every** request. There is no token in `localStorage`.
> - **Auth State**: Check `GET /auth/me` on app load. If it returns 200, the user is logged in. If 401, they are not.
> - **Google Login**: Do NOT use `fetch()` or AJAX for `GET /auth/google`. It requires a full-page redirect.
> - **AI Inputs**: The model expects exactly 11 biomarkers in this specific order: `qNet`, `dvdtmax`, `vmax`, `vrest`, `APD50`, `APD90`, `max_dv`, `camax`, `carest`, `CaTD50`, `CaTD90`.
> - **AI Output**: Read the risk tier from `result[0].label`, not `result[1]`.
> - **Cold Starts**: The production server sleeps after 15 minutes of inactivity. Expect the first request to take 30-60 seconds (implement loading states and retry on `502 Bad Gateway`).
> - **Handle Common Errors**: Write unified handlers for `401`, `403`, `409`, `422`, `429`, and `502`.

---

## Table of Contents

1. [How Authentication Works](#how-authentication-works)
2. [Roles & Permissions](#roles--permissions)
3. [Endpoint Reference](#endpoint-reference)
   - [Email Auth](#email-auth)
   - [Google OAuth](#google-oauth)
   - [Account Management](#account-management)
   - [AI Prediction](#ai-prediction)
   - [Admin Endpoints](#admin-endpoints)
   - [Health](#health)
4. [Error Handling](#error-handling)
5. [Common Pitfalls](#common-pitfalls)
6. [Frontend Pages to Build](#frontend-pages-to-build)

---

## How Authentication Works

This backend uses **Cookie-Based Sessions** instead of JWTs. 

- There is no token to store in `localStorage`. The backend sets an `HttpOnly`, `Secure`, `SameSite=None` cookie upon successful login. It cannot be read in JS.
- **Cross-Origin Requests (CORS)**: Because the frontend and backend are on different domains in production, you must explicitly tell your HTTP client to send the cookie (`SameSite=None;Secure`).
- **HTTPS Required**: Browsers will reject `SameSite=None` cookies if the connection is not secure (HTTPS). Localhost is usually exempted, but production requires it. The frontend origin must be whitelisted.

### The `api()` Fetch Helper

To avoid forgetting the credentials flag, wrap your `fetch` calls in a helper function:

```javascript
// api.js
const BASE_URL = import.meta.env.VITE_API_URL || 'http://localhost:3000';

export async function api(endpoint, options = {}) {
  const url = `${BASE_URL}${endpoint}`;
  const response = await fetch(url, {
    ...options,
    // CRITICAL: This ensures the session cookie is sent with the request
    credentials: 'include', 
    headers: {
      'Content-Type': 'application/json',
      ...options.headers,
    },
  });
  
  return response;
}
```

---

## Roles & Permissions

The platform supports two roles: `user` and `admin`.

When a user successfully logs in, or when you check `GET /auth/me`, the response includes the user's role:

```json
{
  "id": "123e4567-e89b-12d3-a456-426614174000",
  "email": "doctor@hospital.com",
  "email_verified": true,
  "display_name": "Dr. Smith",
  "role": "admin",
  "created_at": "2026-07-10T12:00:00Z"
}
```

**What the frontend should do:**
- Check `role === 'admin'` to show or hide the Admin Dashboard UI in your navigation.
- All endpoints prefixed with `/admin/*` require the user to have the `admin` role. If a normal user attempts to access them, the server returns `403 Forbidden`.

---

## Endpoint Reference

### Email Auth

#### POST `/auth/register`
Creates a new account. Sends a verification email.
- **Access**: Public
- **Body**:
  ```json
  {
    "email": "user@example.com",
    "password": "securepassword123",
    "display_name": "John Doe" 
  }
  ```
- **Success (200 OK)**: Empty body.
- **Errors**:
  - `400 Bad Request`: Invalid email format or password < 8 characters.
  - `409 Conflict`: Email already exists.
  - `422 Unprocessable Entity`: Malformed JSON sent in request body.

#### GET `/auth/verify?token=...`
Verifies a user's email. You don't call this via AJAX. The user clicks the link in their email, which points to the backend. The backend verifies the token and redirects to `FRONTEND_URL/login?verified=1`.

#### POST `/auth/login`
Logs the user in and sets the session cookie.
- **Access**: Public
- **Body**:
  ```json
  {
    "email": "user@example.com",
    "password": "securepassword123"
  }
  ```
- **Success (200 OK)**: Returns the user profile (see `GET /auth/me`).
- **Errors**:
  - `401 Unauthorized`: Invalid credentials, or account is soft-deleted.
  - `403 Forbidden`: Email is not verified yet.

#### POST `/auth/logout`
Destroys the session and clears the cookie.
- **Access**: Protected
- **Success (200 OK)**: Empty body.

#### GET `/auth/me`
Fetches the currently authenticated user's profile.
- **Access**: Protected
- **Success (200 OK)**:
  ```json
  {
    "id": "uuid",
    "email": "user@example.com",
    "email_verified": true,
    "display_name": "John Doe",
    "role": "user",
    "created_at": "2026-07-10T12:00:00Z"
  }
  ```
- **Errors**: `401 Unauthorized` (Not logged in or session expired).

#### POST `/auth/password/forgot`
Triggers a password reset email.
- **Access**: Public
- **Body**: `{ "email": "user@example.com" }`
- **Success (200 OK)**: Returns instantly.

#### POST `/auth/password/reset`
Resets the password using the token from the email.
- **Access**: Public
- **Body**:
  ```json
  {
    "token": "...",
    "new_password": "newsecurepassword123"
  }
  ```
- **Success (200 OK)**: Password changed successfully.
- **Errors**: `400 Bad Request` (Invalid or expired token, or invalid password length).

---

### Google OAuth

#### GET `/auth/google`
Initiates the Google OAuth flow. 
- **Access**: Public
- **Usage**: **DO NOT FETCH THIS ROUTE.** Instead, navigate the browser directly: `<a href="http://localhost:3000/auth/google">Sign in with Google</a>`.
- **Behavior**: The backend redirects the user to Google. Once authenticated, Google redirects back to the backend, which sets the session cookie and redirects the browser to `FRONTEND_URL/login?login=success` (or `?error=auth_failed`).

---

### Account Management

These endpoints use a dual-factor, 6-digit code flow:
1. User requests an action (triggers a 10-minute code sent to their email).
2. User submits the code to confirm the action. If they fail 5 times, the code permanently invalidates.

#### POST `/auth/password/change/request`
Requests a password change for the logged-in user.
- **Access**: Protected
- **Success (200 OK)**: Empty body. The 6-digit code has been emailed.
- **Errors**: `400 Bad Request` (OAuth-only accounts cannot change their password).

#### POST `/auth/password/change/confirm`
Confirms the password change.
- **Access**: Protected
- **Body**:
  ```json
  {
    "current_password": "oldpassword123",
    "new_password": "newpassword123",
    "code": "123456"
  }
  ```
- **Success (200 OK)**: Password updated. Current session remains active, but all other devices are logged out.
- **Errors**: `401 Unauthorized` (Generic error if the current password OR the 6-digit code is wrong).

#### POST `/auth/account/delete/request`
Requests account deletion for the logged-in user.
- **Access**: Protected
- **Success (200 OK)**: Empty body. The 6-digit code has been emailed.

#### POST `/auth/account/delete/confirm`
Confirms account deletion.
- **Access**: Protected
- **Body**: `{ "code": "123456" }`
- **Success (200 OK)**: User is soft-deleted, immediately logged out, and the session is destroyed.
- **Errors**: `400 Bad Request` (Invalid, expired, or locked-out code).

---

### AI Prediction

The backend proxies requests to a Gradio ML space, hiding the complex job-polling architecture from the frontend.

> [!CAUTION]
> The AI expects exactly **11 parameters** in this exact order: 
> `qNet`, `dvdtmax`, `vmax`, `vrest`, `APD50`, `APD90`, `max_dv`, `camax`, `carest`, `CaTD50`, `CaTD90`.

#### POST `/api/predict`
Runs the Cardivex classification model.
- **Access**: Protected
- **Body**:
  ```json
  {
    "data": [
      0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0, 1.1
    ]
  }
  ```
- **Success (200 OK)**:
  ```json
  {
    "data": [
      { "label": "Safe" },
      "Tier 1" 
    ],
    "duration": 0.45
  }
  ```
  > **Note**: Always read the classification tier from `data[0].label`. Ignore the raw string in `data[1]` as it is inconsistently formatted by Gradio. The two-step Gradio flow is hidden by the backend.
- **Errors**: `429 Too Many Requests` (Rate limit exceeded), `502 Bad Gateway` (Gradio space asleep/down).

#### POST `/api/explain`
Generates SHAP values to explain a prediction.
- **Access**: Protected
- **Body**: Same as `/api/predict`.
- **Success (200 OK)**:
  ```json
  {
    "data": [
      {
        "predicted_class": 1,
        "base_value": 0.45,
        "contributions": [
          {"feature": "qNet", "value": 0.12},
          {"feature": "dvdtmax", "value": -0.05}
        ]
      }
    ],
    "duration": 1.2
  }
  ```
- **Errors**: `429 Too Many Requests` (Rate limit exceeded), `502 Bad Gateway` (Gradio space asleep/down).

---

### Admin Endpoints

All admin endpoints require the `admin` role and return `403 Forbidden` for normal users.
List endpoints are paginated. Acceptable query params: `?limit=50&offset=0` (Limit max is 200).
They return a consistent envelope:
```json
{
  "items": [...],
  "limit": 50,
  "offset": 0,
  "total": 125
}
```

#### GET `/admin/users`
Lists all users.
- **Query Params**: `?limit=50&offset=0&include_deleted=true` (optional)
- **Response Items**: User objects with `deleted_at` field.

#### GET `/admin/users/{id}`
Gets a specific user, plus aggregate log counts.
- **Response**: User object merged with `{ "prediction_logs_count": 5, "auth_events_count": 12 }`.

#### DELETE `/admin/users/{id}`
Soft-deletes a user, instantly locking them out.
- **Errors**: `400 Bad Request` (Attempting to delete yourself), `404 Not Found` (User missing or already deleted).

#### POST `/admin/users/{id}/restore`
Restores a soft-deleted user.

#### GET `/admin/auth-events`
Lists global auth events (logins, password changes, deletions).
- **Query Params**: `?user_id=...` (optional filter)

#### GET `/admin/prediction-logs`
Lists global prediction histories.
- **Query Params**: `?user_id=...` (optional filter)

#### GET `/admin/shap-logs`
Lists global SHAP explain histories.
- **Query Params**: `?user_id=...` (optional filter)

---

### Health

#### GET `/healthz`
Checks if the API is awake.
- **Access**: Public
- **Success (200 OK)**: `{"status":"ok"}`

---

## Error Handling

The backend returns standardized HTTP status codes. The response body usually contains a plain text string describing the error (e.g., `"Invalid credentials"`), which you can display directly in a Toast notification.

| Status Code | Meaning | Frontend Action |
| :--- | :--- | :--- |
| **400** `Bad Request` | Validation failed, missing fields, OAuth password change, or invalid/expired 6-digit code. | Show the error text to the user. |
| **401** `Unauthorized`| Not logged in, invalid password, or account deleted. | Redirect to `/login`. |
| **403** `Forbidden` | Email not verified, or missing `admin` role. | Show a "permission denied" or "please verify email" message. |
| **404** `Not Found` | Resource (e.g., user ID) doesn't exist. | Show a 404 UI. |
| **409** `Conflict` | Email already in use during registration. | Ask the user to log in instead. |
| **422** `Unprocessable`| Malformed JSON sent in request body. | Check your API payload format. |
| **429** `Too Many Req`| Hit the rate limit for AI prediction endpoints. | Show "Please wait a moment before trying again." |
| **502** `Bad Gateway` | Gradio ML server is sleeping or down. | Show a loading state, silently retry after a few seconds. |
| **500** `Server Error`| Unexpected backend crash. | Show a generic "Something went wrong" message. |

---

## Common Pitfalls

1. **"I logged in, but the next request says 401 Unauthorized"**
   You forgot `credentials: 'include'` in your `fetch()` call. The browser isn't sending the session cookie.
2. **"Google Login throws a CORS error"**
   You are trying to fetch `/auth/google` via AJAX. You must use `<a href="...">` or `window.location.href = ...` to let the browser handle the redirect.
3. **"The first AI prediction takes 50 seconds"**
   Render (the hosting provider) puts free-tier servers to sleep after 15 minutes of inactivity. Gradio (the ML model host) also sleeps. Build a "Waking up the AI..." loading state in your UI for requests taking longer than 5 seconds.
4. **"The prediction says 'Tier 1' but the JSON is weird"**
   Gradio sometimes formats the second array item as a string instead of JSON. Always read the risk tier strictly from `result.data[0].label`.
5. **"My session doesn't work locally!"**
   Ensure your local frontend and backend ports match your CORS whitelist, and check that you aren't blocking third-party cookies if testing across different local IP addresses.

---

## Frontend Pages to Build

Based on this API, your frontend application should implement:

1. **Auth Pages**:
   - `/login`: Form for email/pass. Button for Google Auth. Handle `?verified=1` (show success banner) and `?error=auth_failed` URL parameters.
   - `/register`: Form for name, email, password.
   - `/forgot-password`: Form to request a reset email.
   - `/reset-password`: Form to submit new password (reads `?token=` from URL).
2. **Main Application**:
   - Dashboard to input the 11 biomarkers and display the tier prediction and SHAP explanation charts.
3. **Account Settings**:
   - Change Password (requires current password + email code UI).
   - Delete Account (requires email code UI + terrifying warning).
4. **Admin Dashboard (Protected)**:
   - User Management Table (list, delete, restore).
   - Global Activity Feeds (auth events, prediction logs, and shap logs).
