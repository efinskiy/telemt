# Linux queue management for Telemt

This optional host service installs one bounded HTB root and an fq_codel leaf
on a chosen Linux egress interface. Telemt continues to enforce priority, bulk,
per-IP and static account/CIDR budgets internally, for Direct, Middle and
accepted WEB traffic. No network privileges are added to the node container.
Requires Python 3.10+, iproute2, systemd, Linux HTB/fq_codel support and root with
CAP_NET_ADMIN. There are no third-party Python dependencies.

## What is shaped

The queue covers **all outgoing packets on the selected interface**, IPv4 and
IPv6, including client downloads, Telegram-bound uploads, fallback, management
traffic and unrelated processes. `egress_bps` must reflect measured usable link
capacity with headroom, not just Telemt payload capacity. With both relay legs on
one interface their egress demand shares this ceiling. Multi-interface hosts
must select the actual outgoing interface; one service owns one interface.

There is no IFB ingress redirection. Client upload budgets remain enforced by
Telemt; fq_codel controls local egress queues only. It cannot remove queues
already inside a provider's network. Select a ceiling below the measured
bottleneck so queuing happens locally. Too small a ceiling limits all traffic;
too large a ceiling leaves latency at the upstream bottleneck.

HTB's only class caps aggregate egress; fq_codel provides per-flow fairness,
ECN marking/drops and bounded queue memory. Semantic priority remains in Telemt,
not in port heuristics or guesses about TLS payloads. No SO_MARK patch is
required. This is an independent host deployment component; Fleet's shaping
page does not claim the kernel service is installed or healthy.

## Installation on Ubuntu 24.04

The repository is assumed to be checked out at `/opt/telemt` on the node host.
If it is elsewhere, adjust the two paths in `telemt-queues.service`.
Do not install this on Fleet unless it is also the intended proxy host.

```sh
sudo apt-get update
sudo apt-get install -y python3 iproute2
ip route get 1.1.1.1
sudo install -d -m 0750 /etc/telemt
sudo install -m 0640 /opt/telemt/deploy/shaping/queues.json.example /etc/telemt/queues.json
sudo editor /etc/telemt/queues.json
sudo python3 /opt/telemt/deploy/shaping/telemt-queues.py check
sudo install -m 0644 /opt/telemt/deploy/shaping/telemt-queues.service /etc/systemd/system/telemt-queues.service
sudo systemctl daemon-reload
sudo systemctl enable --now telemt-queues.service
sudo python3 /opt/telemt/deploy/shaping/telemt-queues.py status
```

`check` must report `ready` before first installation. Existing administrator
qdiscs/filters are rejected rather than overwritten. The example `eth0` and
190 Mbit/s are examples for a measured 200 Mbit/s link; edit before applying.
Defaults: target 5 ms, interval 100 ms, at most 1,024 packets or 4 MiB in the
leaf queue, 4,096 flow buckets, 1,514-byte quantum, 32,768-byte HTB burst.
Choose packet quantum/burst for the interface MTU and measured capacity.
Target is an AQM control value, not a guarantee of end-to-end latency.

The state is `/var/lib/telemt-queues/state.json`, mode 0600 in a 0700 directory.
Commands have a five-second deadline and a 1 MiB output limit; concurrent
lifecycle commands fail rather than wait indefinitely. Root creation uses
exclusive `tc qdisc add` so a concurrent foreign root is not replaced. A failed
installation rolls back only its recognized partial tree. Unknown/conflicting
state is retained for diagnosis. After reboot a matching saved configuration
can reinstall over verified kernel-default queues. An interrupted partial
installation requires inspection and explicit `remove`, then `apply`.

## Telemt policy example

In Fleet, fill the corresponding fields, or use the existing validated config
API. This example allocates 180 Mbit/s to each Telemt payload direction,
40 Mbit/s bulk under contention and at most 140 Mbit/s bulk without contention:

```toml
[shaping]
enabled = true
up_bps = 180000000
down_bps = 180000000
bulk_up_bps = 40000000
bulk_down_bps = 40000000
bulk_idle_up_bps = 140000000
bulk_idle_down_bps = 140000000
priority_percent = 25
bulk_enter_bytes = 262144
bulk_exit_bytes = 65536
sustained_rate_bps = 2000000
sustained_secs = 10
sustained_min_bytes = 8388608
sustained_recovery_secs = 30
main_dc_priority = false
max_ips = 32768
```

The resulting contention reserve is 140 Mbit/s: the explicit bulk cap tightens
it beyond the percentage-derived reserve. All bulk users share each cap.
The two payload directions are independent in Telemt, but may share a single
physical egress queue as explained above. Tune these example values using node
measurements. A 1 Gbit/s port does not establish 1 Gbit/s available capacity.

Sustained state persists across connections/reloads, expires with idle IP
history and resets on process restart. Reducing rate due to adaptive budget
refusals does not count toward amnesty. `main_dc_priority` is optional because
authenticated DC routing is a client-supplied hint, not proof of message content.

## Changes, rollback and diagnostics

Stop the service **before** changing queue configuration. Removal uses the saved
original configuration even if `/etc/telemt/queues.json` has changed. Only an
unchanged owned hierarchy is removed; the kernel restores its default qdisc.
An existing custom hierarchy is never imported or deleted automatically.

```sh
sudo systemctl stop telemt-queues.service
sudo editor /etc/telemt/queues.json
sudo systemctl start telemt-queues.service
sudo python3 /opt/telemt/deploy/shaping/telemt-queues.py status
sudo journalctl -u telemt-queues.service -n 50 --no-pager
```

To roll back host queuing:

```sh
sudo systemctl disable --now telemt-queues.service
sudo python3 /opt/telemt/deploy/shaping/telemt-queues.py status
```

Separately disable Telemt shaping or set `sustained_secs = 0` and
`main_dc_priority = false` to retain the earlier flow classification. Setting
both idle caps to zero restores full idle borrowing. When downgrading to an
older Telemt binary remove the seven new fields first: its strict parser rejects
unknown fields. Never erase the ownership file to bypass a conflict.

| Symptom | Check / cause |
| --- | --- |
| `foreign queue hierarchy present` | Existing tc policy, ingress qdisc or filters; inspect with `tc -d -s qdisc/class/filter show dev INTERFACE`. |
| `configuration differs` | Configuration changed while active, partial installation or external edits. Inspect `status`; explicit remove is possible only for a verified owned tree. |
| `Operation not permitted` | Service lacks CAP_NET_ADMIN or is inside the wrong network namespace. Run the service on the host. |
| `Specified qdisc kind is unknown` | Kernel lacks `sch_htb` or `sch_fq_codel`; install/load matching kernel modules. |
| All traffic slows | Physical egress ceiling too low or both relay directions exceed the shared link ceiling. |
| Latency remains high | Ceiling above provider bottleneck, different egress interface or provider ingress queue. |

## Verification

Unit lifecycle checks do not modify networking:

```sh
python3 -m unittest discover -s deploy/shaping/tests -v
```

On a disposable Linux host with `iproute2`, `iputils-ping` and NET_ADMIN/network
namespace permissions, the opt-in test creates two temporary namespaces, installs
the real hierarchy, sends packets, verifies counters and removes it. It never
changes a physical host interface:

```sh
sudo env TELEMT_QUEUE_NETNS_TEST=1 python3 -m unittest discover -s deploy/shaping/tests -v
```

The namespace test must be run before rollout. Local development on macOS cannot
verify Linux kernel behavior or latency under real load. Start with one node and
measure queue drops/ECN, small-request p95, bulk throughput and API reachability.
