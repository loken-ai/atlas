# Changelog

All notable changes to this project are documented here, in the format of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- `ClusterSnapshot`, and wire types that keep the distinctions the server draws: a rate of
  zero is unknown, an absent catalogue is not an empty one, and an absent energy figure makes
  any total partial.
- `doctor`, eight rules as pure functions, each with a test and a negative control.
- `metrics`, an OpenMetrics encoder for what only an observer measures. Every rule keeps a
  series at zero so a rule that stops firing cannot be confused with an exporter that broke.
- `atlas` and `atlas-gui`, sharing the core.

- `collect`, polling `/health`, `/api/cluster/state`, `/api/cluster/peers`,
  `/api/distributed/devices` and `/api/models/loaded`. Endpoints are asked one at a time per
  node, and the round trip is taken on the request already being made.
- `atlas watch`, `atlas doctor` and `atlas export` all run against real nodes.

### Not yet

The TUI is a printed listing rather than a ratatui view, and there is no multicast listening,
so a foreign cluster is not yet detected.
