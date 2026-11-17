# crates/edge/config

## Purpose

The one way configuration is read: `Config::load` finds the file (argument,
`TIMBRE_CONFIG`, `./timbre.toml`), and `Config::section` fills a module's
typed section from it, applying `TIMBRE_<SECTION>__<KEY>` environment
overrides over the file over `Default`. A missing file is defaults; an
unreadable or invalid one fails startup naming the file and line.

## Ports

None.

## Signals

None.

## Failures

- `config.invalid` (`ConfigFailure::Invalid`): the config file exists but
  cannot be read, parsed or deserialized into the requested section, or an
  environment override value cannot be applied. The reason names the file,
  line, section and key but never the offending value itself (`<value>`):
  config values can be secrets and the reason reaches the log. Every serde
  value shape is hidden — typed values (``integer `…` ``, ``string "…"``),
  an unrecognized enum spelling (``unknown variant `…` ``) and a value a
  field's own validation rejected (``invalid value: `…` ``) — while the
  expected-variant list and the key context stay readable.
