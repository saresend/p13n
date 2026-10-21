# intelligence-engine

The personalization server. Apps send it every action a user takes; it watches for patterns and
streams back suggestions for what to do next. Spec: Notion → Backlog → Personalization System for
UX → Personalization Server.

```sh
cargo run                 # http://127.0.0.1:8080
cargo test
cargo clippy --all-targets
```

| env               | default                 |                                               |
|-------------------|-------------------------|-----------------------------------------------|
| `BIND`            | `127.0.0.1:8080`        | listen address                                |
| `DATABASE_PATH`   | `data/engine.sqlite`    | SQLite file (created if missing)              |
| `TICK_MS`         | `5000`                  | how often each user's worker checks           |
| `MIN_NEW_ACTIONS` | `5`                     | new actions needed before the model runs      |
| `CORS_ORIGINS`    | `http://localhost:5173` | comma-separated browser origins (text editor) |
| `RUST_LOG`        | `info`                  | log level                                     |

## API

**`POST /init`**: body is a JSON Schema describing valid `{ action_name, payload }` objects, the
model's vocabulary. Replaces the previous one and is persisted. `400` if the schema is invalid.
`examples/text-editor.schema.json` is the vocabulary for `../text-editor`.

```sh
curl -H 'content-type: application/json' --data @examples/text-editor.schema.json localhost:8080/init
```

**`POST /action`**: one action. `202 {"id": n}` on success, `409` before any `/init`, `422` with
`details` if `{ action_name, payload }` doesn't match the vocabulary.

```json
{ "user_id": "sam", "session_id": "tab-1", "action_name": "save",
  "payload": { "id": "note-1", "text": "…" }, "source": "user" }
```

`source` is `user`, `agent` or `system`. `payload` is anything the vocabulary allows.

**`GET /sse/suggestion?user_id=sam`**: server-sent events. Each suggestion arrives as:

```
event: suggestion
data: {"confidence":0.0,"user_id":"sam","actions":[{"action_name":"save","payload":{…},"source":"system"}]}
```

Comment lines (`: keepalive`) are sent every 15s. In a browser:

```js
new EventSource('http://localhost:8080/sse/suggestion?user_id=sam')
  .addEventListener('suggestion', (e) => console.log(JSON.parse(e.data)))
```

**`GET /health`**: `{"ok": true}`.

## How it works

1. `POST /action` validates the action, stores a row in SQLite (`actions` table), and, the first
   time it sees a `user_id`, starts a worker thread for that user.
2. Every tick (5s), the worker counts the user's actions newer than the last one it processed.
   With at least 5, it calls `model.predict(schema, actions)` on the newest of them (at most 100),
   drops proposed actions that don't match the vocabulary, records the newest action as
   processed, and pushes a `Suggestion` onto the user's queue.
3. The handoff to SSE is per user. Every open stream for that user receives each suggestion. If
   none is open, suggestions wait in a buffer (32 deep; when full, the oldest is dropped) and go
   to the next stream that connects. Fan-out is deliberate: a closed tab's stream stays open on the
   server until a write fails, and it must not take suggestions meant for the tab that replaced it.

The "last processed" position lives in memory, as in the spec. After a restart, a user's first
run covers their newest 100 actions.

## Code

- `src/api.rs`: routes and error responses.
- `src/engine.rs`: shared state, worker threads, and `step` (one worker tick).
- `src/model.rs`: the `DecisionModel` trait and the `IdentityModel` stub. See `MODEL.md`.
- `src/db.rs`: SQLite schema and queries (parameterized).
- `src/vocabulary.rs`: JSON Schema validation.
- `src/queue.rs`: the suggestion queue.
- `tests/api.rs`: API, worker, persistence, and SSE round-trip tests.

## Not yet

- No authentication: anyone who can reach the port can read any user's suggestions. Keep it on
  localhost (the default) until there is auth.
- One thread per user is fine for a personal deployment, not for many users.
- The text editor doesn't send its events here yet. Its event shape (`action`, `payload`, `at`)
  needs a small adapter to this one (`action_name`, `payload`, plus `user_id`, `session_id`,
  `source`).
