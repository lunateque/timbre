# timbre-auth_guard

## Purpose

Brute-force limiting. Before checking a credential the kernel reserves space
in both applicable sliding windows (client address and named user). Only
failed checks count after settlement. IPv6 clients are counted per /64,
because a single host is routinely handed every address in one.

The number of remembered windows is capped (`max_tracked`, 100000 by
default): an attacker who sends one failure per address would otherwise grow
the table without bound, and a limiter that exhausts the server it protects
is worse than none. Each window holds at most one timestamp per failure it
allows, so a configuration must also keep
`max_tracked * (address_failures + user_failures)` at or below 20000000
timestamps, roughly 320 MB; the per-setting ranges alone do not bound that
product. The least recently touched eligible window is evicted
first. A window with a check in flight or with a full quota still cooling
down cannot be evicted. If none can make room, unfamiliar keys are admitted
without tracking. Once the window elapses, the kernel's minute tick or the
next attempt reports their total as `auth_guard.table_full` regardless of
the log filter.

Leaving this module out of the binary removes the kernel's throttle
entirely, which is what a server behind somebody else's limiter wants.

## Ports

Provides the kernel's `Throttle`. Uses `timbre_edge_config::Config` for the
`[auth_guard]` section; without it the defaults apply.

## Signals

None. Settings are validated and retained by the kernel startup check.

## Failures

`auth_guard.invalid_config` when `[auth_guard]` has a value outside the
supported ranges: 1–10000 failures per address or user, 1–1440 minutes, or
1–1000000 tracked windows, or when their product exceeds the timestamp
bound above.
