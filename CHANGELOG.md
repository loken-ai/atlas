# Changelog

All notable changes to this project are documented here, in the format of
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- Model: ClusterSnapshot and wire types keep the server's distinctions: unknown is not zero, absent is not empty.
- Doctor: eight consistency rules as pure functions, each with a test and a negative control.
- Metrics: an OpenMetrics encoder for what only an observer measures, every rule held at zero when idle.
- Binaries: atlas and atlas-gui, sharing one core.
- Collect: staged polling of health, cluster state, peers, devices and loaded models, one endpoint at a time.
- CLI: watch, doctor and export, running against real nodes.
- Watch: a ratatui view naming each node by its id, with its devices, resident models and in-flight requests.
- Discovery: passive multicast listening, so a foreign cluster on the same network is detected.
- Watch: paints from the first frame and polls in the background, so a slow node never holds the screen.
- Watch: GPU and CPU utilization on every device row, and decode and prefill throughput per node.
- Watch: a streamed model's per-card residency in gigabytes, and a model shown as loading while it warms.
