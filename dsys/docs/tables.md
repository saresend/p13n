Set up a CRUD api against the following two tables.

# Action Table

This table will represent any action I take across a set of applications. The idea is that this table will be a
system of record for any action I take in some of the applications.

This table should be similar to the one in intelligence-engine, with the following changes

1. There should be no "session" concept. Actions will abstractly be the following:

(id (pk), source_application: str, action_name: str, payload: str (this would be json parseable parameters into the action), timestamp)

# Tasks Table

See kyogre sqlite artifact in claude for reference to the exact table format. The API around this should include reading / writing file storage for implementing task management logs.

## Technology

1. actix_web for API layer
1. Diesel for sqlite stuff
