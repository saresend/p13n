CREATE TABLE action (
  id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
  source_application TEXT NOT NULL,
  action_name TEXT NOT NULL,
  payload TEXT NOT NULL,       -- JSON-parseable parameters for the action
  timestamp BIGINT NOT NULL    -- Unix epoch milliseconds
);

CREATE INDEX action_source_timestamp ON action (source_application, timestamp);
CREATE INDEX action_timestamp ON action (timestamp);
