# Independent maintenance egress (proposal + implementation)

## Scope and compatibility

Account balance (`usage.query`), check-in (`checkin.claim`), and reward status/claims
(`claim.status`, `claim`) get an opt-in, operation-local egress policy. No account
proxy fields, model requests, login defaults, process proxy variables, or other
providers are rewritten. Existing behavior is preserved with the policy disabled.
New accounts inherit the current policy at invocation; there is no account migration.

This needs **one compatible host upgrade**. A new plugin alone cannot give an old
host this management API or scheduler. After that, providers can opt into the same
versioned capability through plugin updates. Unsupported plugins fail closed with
an upgrade-required error, rather than silently sending a direct request.

## UI and API

Web/desktop Network Proxies includes a separate maintenance policy card.
Authenticated `GET/PUT /api/maintenance-policy` reads/replaces the complete policy.
The response contains settings and active/pending counts, not proxy credentials.
The dedicated mobile UI is not added in this change; settings can be managed from
the web panel. Saving does not restart the server.

- `direct`: explicit direct; ignores account, system and gateway defaults for the scoped operation.
- `fixed`: references one existing, enabled native proxy-pool ID.
- `auto`: round-robin among enabled, resolvable native pool entries, with per-entry
  concurrency and transport-error cooldown. No implicit direct fallback.
- The fixed selection is an **entrance**, not a promise of one public IP. An
  external aggregation service can rotate its own downstream nodes behind it.
- Subscription URLs are not proxy endpoints. This change does not restore retired
  subscription parsing. An external compatible HTTP/SOCKS5 aggregator can be stored
  as one manual entry; individually exposed listeners can be separate pool entries.

Only IDs are persisted in the policy. Native pool credentials are resolved at
admission and sent to the plugin on private stdio, never returned by this policy API.

## Scheduling and defaults

Opt-in default: disabled; providers `raccoon, loomy`; all four maintenance operations.
When enabled: global concurrency 2, per-entrance concurrency 1, random start spacing
500–1500 ms, at most 128 queued tasks, queue timeout 120 s, successful balance cache
60 s, transport cooldown 30 s. Limits are validated server-side.

All selected operations share one admission coordinator. Tasks for the same
provider/account serialize, including simultaneous balance/check-in/reward actions.
A dropped queued task removes its pending count; a dropped running permit releases
both account and concurrency slots. Starts are spaced globally, rather than letting
independently jittered workers all wake at once. Queue-full/queue-timeout returns a
bounded error; it does not claim an upstream success. Waiting is outside RPC network
timeout. Only successful balance results are cached, and mutation attempts invalidate
that account's cached balance even if their responses are lost.

Auto mode skips deleted/disabled/unresolvable entries at admission. Typed transport
errors and RPC timeouts cool an entrance for future tasks. Authentication/business
rejections do not mark a proxy as broken. No check-in/reward is automatically replayed:
a timeout may mean the remote mutation succeeded. Round-robin/cooldown affect future
operations, not an invisible second attempt of the current mutation. Partial reward
results remain provider-defined and are not converted into a successful retry.

## Optional RPC contract

`maintenance.capabilities` with `{provider}` returns
`{version:1, operations:[...]}`. Only explicitly opted-in providers advertise support.
For one supported RPC, host adds `maintenanceEgress: {version:1, proxy:null|WireProxy}`.
Missing envelope preserves legacy behavior; `proxy:null` means explicit direct.
Unknown versions, malformed proxies and use on chat/model RPCs are rejected.
The SDK holds the envelope in task-local scope, not `PluginCtx.env` or global state.
Raccoon and Loomy maintenance HTTP sites consume it; chat/login paths do not.
Transport failures are reported with `maintenance_transport`, without endpoint or
credential details. Third-party providers must opt in only after implementing all
advertised operations, including nested helpers.

## Verification and limitations

Tests use synthetic accounts and loopback-only upstream/proxy fixtures, never real
service tokens or reward claims. SDK tests cover scope isolation and validation;
real-plugin RPC tests cover explicit direct, authenticated HTTP/SOCKS5, legacy behavior,
invalid/dead proxies and rejection of maintenance envelopes on model dispatch.
Host tests exercise concurrency, queue bounds/cancellation, randomized spacing,
per-account serialization, balance cache, new-account admission and cooldown.
This is not evidence of real provider availability or a fixed public exit IP.

The feature is default-off and must not be automatically enabled during upgrade.
To disable it, save `enabled:false`; subsequent calls use the original path. Already
admitted operations retain their snapshot. Never roll back a production account DB
just to remove this feature. Upstream release/signing is left to maintainers.

## Reproducing checks

- Plugin: `cargo test -p agent2api-provider-sdk --lib`; build the provider binary, then
  `unshare --net sh -c 'ip link set lo up; python3 tests/maintenance_egress.py BINARY'`.
- Host: `cargo check -p agent2api-server --lib` from the Rust workspace;
  `cargo test --manifest-path scripts/maintenance-scheduler-tests/Cargo.toml` from repository root.
- End-to-end (Linux, isolated namespace):
  `unshare --net sh -c 'ip link set lo up; python3 scripts/test-maintenance-runtime.py HOST_BINARY PLUGIN_BINARY PLUGIN_FIXTURE_SCRIPT HOST_UI_DIR'`.
  This creates a disposable authenticated host, installs a synthetic test pack,
  imports only fake accounts, and tests policy persistence and routing.
- Web UI: install locked dependencies in both `desktop-tauri/ui-kit` and
  `desktop-tauri/ui-islands`, then run the islands build (includes TypeScript checks).
