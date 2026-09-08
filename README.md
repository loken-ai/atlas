# atlas

Watch a loken cluster: what each node holds, what it measures, and where they disagree.

```
cargo build --release
```

## What it does

Three tools over one collector.

- **`atlas watch`** - the living map. Nodes, their devices, what is loaded, what is in flight,
  and the round trips between them.
- **`atlas doctor`** - the inconsistencies a cluster swallows in silence. Exits with the number
  of rules that fired.
- **`atlas export`** - OpenMetrics for the facts no single node holds: round trips as the
  observer measures them, membership, versions, and doctor's verdicts.

`atlas-gui` draws the same state and takes the same flags. Two views of one cluster, not two
tools: a second collector would mean a second model of the cluster, and two models drift.

![atlas watch](docs/img/atlas-tui.svg)

![atlas-gui](docs/img/atlas-gui.png)

Both images are rendered by the test suite, driving the same code the binaries run, from a
snapshot written in `src/screenshots.rs` - so they show the current layout and can show no real
address, host or model. Regenerate them with
`cargo test --release --all-features screenshots -- --ignored`.

Node addresses come from `--node`, from a config file, or from listening. None are compiled in.

## Where it is careful

**atlas never announces itself.** A probe that joins the discovery group becomes a peer the
router can hand work to.

**`/api/cluster/state` is polled slowly.** The round trip of that request is what the router
uses to price a hand-over, so hammering it inflates the measured latency of the node it
describes and biases routing away from a peer that is fine. An observer that changes what it
observes is not observing. `/api/stage_perf` is never polled at all: it is a global toggle that
resets counters.

**`0.0` means never measured, not zero.** Every rate a node publishes uses zero for unknown -
the server says so explicitly, because when both nodes report zero every candidate ties at
infinity. Printing `0 tok/s` states a number the engine went out of its way not to mean, so
atlas prints `-`.

**`serves: null` is not `serves: []`.** Null is a node that did not say; empty is a node that
serves nothing. Collapsing them makes a weightless node look like an unknown one.

**A partial energy total says so.** Where reporting is off on any node, no cluster total is
emitted rather than a sum that reads as complete.

**The round trip is taken on the request already being made**, never on a separate ping, which
would measure a path the router does not price.

## Licence

MIT or Apache-2.0, at your option.
