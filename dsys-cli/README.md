# dsys-cli

Command-line client for the [dsys](../dsys) API: actions, tasks (with their
description and progress-log files) and the file tree that stores notes.

```sh
cargo install --path .
```

## Log in

dsys running on an exe.dev VM sits behind exe.dev's proxy, which needs a token.
Create one, then save it:

```sh
ssh exe.dev ssh-key generate-api-key --vm <vm>          # prints a token
dsys-cli auth https://<vm>.exe.xyz <TOKEN>             # 1. server URL, 2. token
# or leave the token out to paste it when asked (keeps it out of shell history):
dsys-cli auth https://<vm>.exe.xyz
echo "$TOKEN" | dsys-cli auth https://<vm>.exe.xyz
dsys-cli auth --status                                 # shows what's saved and checks it
dsys-cli auth --logout                                 # forgets the token
```

The token is sent on every request as `X-Exedev-Authorization: Bearer <token>`.
The exe.dev proxy checks it and strips it before the request reaches dsys.
It is saved with the server URL in `~/.config/dsys-cli/config.json`, readable only by you
(`DSYS_CLI_CONFIG` moves it). For a single command, `--server` / `DSYS_URL` and
`--token` / `DSYS_TOKEN` override the saved values. With nothing saved, the server
is `http://127.0.0.1:8081` and no token is sent.

## Commands

```sh
dsys-cli health

dsys-cli actions list [-s APP] [-n NAME] [--since MS] [--until MS] [--limit N] [--offset N]
dsys-cli actions get ID
dsys-cli actions create -s APP -n NAME [-p JSON|@file|-] [--timestamp MS]
dsys-cli actions update ID [-s APP] [-n NAME] [-p JSON] [--timestamp MS]
dsys-cli actions delete ID

dsys-cli tasks list [--status Available|In-Progress|Complete] [--model M]
dsys-cli tasks get ID
dsys-cli tasks create -t TITLE [-d TEXT | --description-file FILE] [--model M]
dsys-cli tasks update ID [--status S] [--title T] [--model M] [--taken-by-pid N] [--taken-by-start N]
                         [--expected-status S]     # claim safely: fails with 409 if it changed
dsys-cli tasks delete ID [--keep-files]
dsys-cli tasks description ID          |  dsys-cli tasks set-description ID [FILE]   # default stdin
dsys-cli tasks log ID                  |  dsys-cli tasks set-log ID [FILE]
dsys-cli tasks log-append ID [TEXT]    # default stdin

dsys-cli files tree [PATH] [--plain] [--content] [--hidden]
dsys-cli files get PATH                # raw bytes to stdout
dsys-cli files put PATH [FILE] [--create-only | --if-match ETAG]   # default stdin
dsys-cli files rm PATH [--if-match ETAG]
dsys-cli files mkdir PATH
dsys-cli files rmdir PATH [-r]
dsys-cli files mv FROM TO
```

JSON responses are pretty-printed; file, description and log contents are printed
as-is, so they can be piped. Errors go to stderr with a non-zero exit code.
