# Rusty Feed native API

Article URLs returned by both native and compatibility APIs use the effective
instance tracking-parameter policy. The native detail resource also returns the
publisher-provided URL separately as `original_url`/`publisher_url`.

The native API is versioned under `/api/v1`. It uses the same revocable tokens
created in **Account → API tokens**. Send a token only over HTTPS:

```http
Authorization: Bearer <token>
Accept: application/json
```

`X-Auth-Token: <token>` is also accepted. Native requests are bounded to 600
per token per minute. The machine-readable contract is available at
`/api/v1/openapi.json` whenever `API_ENABLED=true`.

Tokens created by the current UI have full personal API access. Internally,
empty scopes mean full access; integrations provisioned with explicit scopes
may use `read`, `write`, or `api`. Every resource is scoped to the token owner.
Shared feed/article rows are never duplicated by API use.

## Resources

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/v1/user` | Current account identity |
| `GET` | `/api/v1/feeds` | Personal subscriptions with shared source health |
| `POST` | `/api/v1/feeds` | Subscribe to `{url, name?}` and queue one shared refresh |
| `DELETE` | `/api/v1/feeds/{id}` | Remove only the caller's subscription |
| `POST` | `/api/v1/feeds/{id}/refresh` | Queue a deduplicated shared refresh |
| `GET` | `/api/v1/articles` | Paginated Library (`feed_id`, `unread`, `limit`, `offset`) |
| `GET` | `/api/v1/articles/{id}` | One authorized article, content, summary, and personal state |
| `POST` | `/api/v1/articles/{id}/state` | Set `read` and/or `bookmarked` booleans |
| `GET` | `/api/v1/saved-searches` | List personal saved searches |
| `POST` | `/api/v1/saved-searches` | Create `{name, query, notify}` and queue its first match pass |
| `DELETE` | `/api/v1/saved-searches/{id}` | Delete a personal saved search |
| `GET` | `/api/v1/categories` | List personal folders and counts |
| `POST` | `/api/v1/categories` | Create `{name, parent_id?}` |

Collections use `{ "data": [...], "meta": {...} }`; individual resources use
`{ "data": {...} }`. Errors use `{ "error": "safe message" }` with standard
HTTP status codes. Limits are bounded server-side and invalid/foreign resource
IDs return `404` without revealing another user's data.

Example:

```bash
curl -H "Authorization: Bearer $RUSTY_FEED_TOKEN" \
  "https://reader.example/api/v1/articles?unread=true&limit=25"
```

## Google Reader compatibility

Existing mobile clients can use the implemented GReader subset. Configure the
server/base URL as the Rusty Feed origin; the protocol routes are the conventional
`/accounts/ClientLogin` and `/reader/api/0/*` paths, not `/api/v1`.

`ClientLogin` accepts a local account password or an existing app API token and
returns a revocable auth token. Subsequent reads use:

```http
Authorization: GoogleLogin auth=<token>
```

Mutations also send the short-lived value returned by
`GET /reader/api/0/token` as form field `T`.

| Method | Path | Supported behavior |
|---|---|---|
| `POST` | `/accounts/ClientLogin` | Local/app-token login and GReader token minting |
| `GET` | `/reader/api/0/token` | Mutation token |
| `GET` | `/reader/api/0/user-info` | Account identity |
| `GET` | `/reader/api/0/subscription/list` | Personal subscriptions and folders |
| `POST` | `/reader/api/0/subscription/edit` | Subscribe, unsubscribe, or rename |
| `GET` | `/reader/api/0/tag/list` | Personal folders plus read/star states |
| `GET` | `/reader/api/0/stream/items/ids` | Bounded IDs, filters, ordering, continuation |
| `POST` | `/reader/api/0/stream/items/contents` | Selected authorized item bodies |
| `GET` | `/reader/api/0/stream/contents/{stream}` | Feed/state/folder stream contents |
| `POST` | `/reader/api/0/edit-tag` | Read/unread, star/unstar, folder mapping |
| `POST` | `/reader/api/0/mark-all-as-read` | Mark a bounded stream read before a timestamp |

Compact numeric aliases are stable database-managed protocol identifiers;
UUIDs remain canonical. Every content read proves the token owner subscribes
to the article's shared feed, and all mutable state stays personal. Replay
integration tests cover login, subscriptions, streams, read state, and stars.
New custom integrations should still prefer the clearer versioned native API.

## Fever compatibility

Create a named Fever username/password under **Account → API tokens** and use
the Rusty Feed origin in a Fever-compatible client. Rusty Feed stores only a hash of
the protocol's `md5(username:password)` key; the credential is independently
revocable in the token list. `/fever/` supports JSON groups, feeds, favicons,
items, unread/saved ID synchronization, and read/unread/save/unsave mutations.
It deliberately omits Fever links, sparks, and kindlings, matching Miniflux's
documented compatibility scope. Set `FEVER_API_ENABLED=false` to remove the
endpoint and credential form without disabling the native or GReader APIs.
