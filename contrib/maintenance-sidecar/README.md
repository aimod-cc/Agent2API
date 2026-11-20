# Optional maintenance sidecar reference

This is a source reference for an independently operated Loomy/Raccoon maintenance
worker and its dashboard. It is deliberately not enabled by the default Agent2API
image, Compose file, plugin registry or UI. No existing production configuration is
changed by checking out this directory.

See [the implementation report](../../docs/maintenance-sidecar.md) for implemented
behaviour, integration contracts, limitations and isolated tests. Installation
status is recorded in [the installation guide](../../docs/install.en.md).

The main binary and model forwarding code remain unchanged. Balance-cache
synchronisation still requires an opt-in usage bridge around the official provider
plugin; "unmodified host" does **not** mean "no integration needed". The bridge
source is provided for review, with no automatic plugin replacement script.

The standalone Dockerfile builds only this source. It does not contain account
data, proxy subscriptions, operator keys, provider binaries or bridge configuration.
Runtime database/key/auth/bridge-key mounts and a compatible official provider
binary are operator-supplied. `MAINTENANCE_GATEWAY_URL` selects the management URL;
other `MAINTENANCE_*` paths/version/target values are declared in `server.py`.
Default startup is paused and automatic operations are disabled.

Current validation targets Linux amd64 and the documented provider/database
contract. Treat this as a draft reference, not a cross-version drop-in installer.
Database writes for check-in and renewable credentials, encrypted secret callbacks,
capacity rejections and stale-balance handling remain explicit review points.

Tom Select 2.4.3 is vendored under its MIT license; see `vendor/` and
`vendor-manifest.json`. The bridge uses only the Go standard library.
