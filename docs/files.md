# Files

Where mxt keeps its session, stores, caches and logs, and what it does when
the session cannot be used any more.

## Directories

| Directory | Default | Contents |
|---|---|---|
| data, `$XDG_DATA_HOME/mxt` | `~/.local/share/mxt` | `session.json` (the homeserver, user ID, device ID and access token), `lock`, and `store/`, the SDK's state and encryption stores |
| cache, `$XDG_CACHE_HOME/mxt` | `~/.cache/mxt` | the SDK's event cache and media cache, which hold decrypted messages and media |
| state, `$XDG_STATE_HOME/mxt` | `~/.local/state/mxt` | the logs, and `history`, the history of the `:` command line and the `/` search lines |

The defaults apply on macOS too, and also when a variable is empty or a
relative path. `HOME` must be an absolute path. mxt creates the directories
with mode 0700 and tightens existing ones to 0700; `session.json`, `lock` and
`history` have mode 0600. The stores have no passphrase: the permissions
protect them. Instances that share the state directory share one `history`
(the lock is per data directory), and the last to save wins.
The cache directory must not contain the data or state directory, because
mxt deletes the cache directory when it sets data aside (below). Downloads
go to `files.download_dir`, not to the cache.

Only one mxt can use a data directory at a time, because two processes must
never share the encryption store. A second one exits with
`mxt: another mxt is running (lock: <data directory>/lock)`.

## Logs

Logs are written to daily files `mxt.<date>.log` (dates in UTC), and the
last seven are kept; nothing is logged to the terminal. `MXT_LOG` takes a
[tracing `EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
directive, for example `MXT_LOG=debug` or `MXT_LOG=warn,mxt_matrix=debug`.
The default is `warn,mxt=info,mxt_core=info,mxt_matrix=info,mxt_tui=info`.
Failed requests to the homeserver, and failures of the opener, the external
editor and image display, are also logged at `warn`, so the log keeps the
errors the status line has replaced. So are the configuration's warnings,
each with its file and line (see
[Configuration](configuration.md#errors-and-warnings)). `:messages`
lists the status line's recent messages of the running mxt.

## When the session ends

When the session cannot be used any more, mxt moves its data aside, prints
why, and exits; the next start logs in as a new device, and asks for the
recovery key again:

- The homeserver answers `M_UNKNOWN_TOKEN`, for example after the device was
  signed out from another client: mxt restores the terminal, deletes the
  cache directory, and renames the data directory to
  `mxt.invalid-<unix seconds>` next to it.
- `session.json` exists but `store/` does not: a new store would give the
  existing device new encryption keys, so mxt does the same. The old device
  stays in the account's device list.

## Switching accounts

`session.json` belongs to one account. If `account.user_id` names another
one, mxt stops and names both. Deleting `session.json` switches accounts: the
next start moves the old `store/` to `store.invalid-<unix seconds>` in the
data directory, empties the cache directory and logs in. The homeserver in
`session.json` is used even when `account.homeserver` changes.
