# timbre-auth

## Purpose

Users, their permissions, and the device secrets that speak for them. The
module stores users in `auth_users`, their granted permissions in
`auth_user_permissions` and one row per issued device in `auth_devices`,
answers `CredentialKind::ApiKey` checks for the kernel, and owns the `user`
and `device` commands.

List commands write to stdout regardless of the log filter. The directory
skips an invalid stored account name and warns once per name per process.
Device labels contain 1–64 characters (no Unicode controls or format
characters); default and explicit expiry are 1–36500 days.

A key reads `<prefix>_<secret>`: thirteen base32 characters that find the
row, an underscore, and fifty-two that prove it. Only the SHA-256 digest of
the whole key is stored, so the database alone cannot authenticate anybody.
Every completed verification path does one digest and one constant-time
comparison, including malformed keys and unknown prefixes. The device
lookup and its user's permission reads share a SQLite snapshot.

`device issue` prints the secret once, after the row is committed: a write
that cannot reach stdout (a closed pipe, a full disk) leaves the device
issued with a secret nobody holds. `device reissue <id>` replaces that
secret and prints the new one; `device revoke <id>` removes the device.

## Ports

- Provides `timbre_kernel::directory::Directory`: lists users and devices for
  anything that needs to show them.
- Provides the kernel's `CredentialProvider` for `CredentialKind::ApiKey`.
- Uses `timbre_edge_sqlite::SharedDatabase` and `timbre_edge_config::Config`;
  without either, registration still succeeds and the commands report
  `auth.not_configured`.

## Signals

Publishes `UserAdded`, `UserRemoved`, `PermissionsChanged`, `DeviceIssued`
(the only signal that carries the one-time secret) and `DeviceRevoked`.
Signals identify users by canonical name. Revoking a stored permission is
allowed even when no registered module still declares it. Subscribes to nothing.

## Failures

`kernel.usage` for malformed commands; `auth.invalid_config`,
`auth.stored_user_invalid`, `auth.not_found`,
`auth.exists`, `auth.unknown_permission`,
and `auth.not_configured`.
The `Directory` port pages its lists and uses `kernel.page_too_large` for
oversize pages and `kernel.cursor_invalid` for malformed cursors; the CLI
lists take no paging arguments and walk every page themselves, in one
read transaction.
