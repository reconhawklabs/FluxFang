# randomized-persist

Working notes toward re-linking randomized addresses across rotation, so
that stalking/following detection keeps working against devices that
randomize. **Not implemented.** This file is the design scratchpad; nothing
here is wired into the product yet.

Purpose is defensive: FluxFang exists to detect someone following you.
Every modern phone randomizes its Wi-Fi probe MAC, which means the devices
most likely to belong to a person following you are exactly the ones the
current co-travel detector cannot see. See "Why" below for the measurements.

Prerequisite already shipped: `MacPersistence`
(`fluxfang-core/src/classify.rs`) splits addresses into `stable` /
`per_network` / `session` / `ephemeral` / `unlinkable`, so the work below
only has to target the `ephemeral` and `unlinkable` classes — the other
three are already trackable as-is.

---

## Why (measurements, 3-day capture, 40,799 emissions / 9,375 emitters)

Co-travel scoring requires `points >= 2` (≥2 distinct spatial cells), and
weights spread 45% / points 35% / span 20%.

| Emitter type | Emitters | Co-travel eligible | Avg span |
|---|---|---|---|
| wifi_client, randomized | 1,107 | **1.1%** | **0.3 min** |
| wifi_client, not randomized | 395 | 15.4% | 1.0 min |
| bluetooth, randomized | 2,818 | 7.1% | 2.1 min |
| bluetooth, not randomized | 259 | 17.4% | 4.4 min |
| wifi_access_point | 4,260 | 15.3% | 5.8 min |

A randomized Wi-Fi client is observable for a median of well under a minute
and clears the scoring gate 1.1% of the time — 14× worse than a
non-randomized one. The clients that *do* score skew toward IoT, embedded,
and older hardware, i.e. the population least likely to be a person.

44.5% of the emitter table is randomized. 68% of randomized wifi_clients
are single-hit rows.

---

## Rec 3 — capture linkage features at ingest

**This is the blocker for everything else, and it is time-sensitive: these
fields cannot be reconstructed after the fact.** Every emission captured
before this ships is permanently unclusterable. `wifi/parse.rs` currently
extracts SSID and the RSN/WPA IEs and nothing else.

Candidate signals, roughly in order of value per unit of effort:

- **802.11 sequence number.** 12-bit, monotonic per transmitter, wraps at
  4096. Sits at offset 22–23 of the MAC header (`seq_ctrl >> 4`);
  `MAC_HEADER_LEN` already skips past it, so this is a two-line change. A
  MAC change mid-stream with continuous SN is a near-certain link. Cheapest
  and strongest short-window signal available.
- **Probe-request IE fingerprint.** Hash over the ordered tag-ID list plus
  the HT/VHT/HE capability bytes, extended capabilities, and any
  vendor-specific IEs. ~5–8 bits of entropy on a mixed population — not
  unique, but a strong clustering feature combined with the rest. Store the
  hash *and* the raw ordered tag list (the hash alone can't be re-derived
  under a changed algorithm).
- **Timing / channel-sweep pattern.** Inter-burst interval and the channel
  order a device sweeps are stable per chipset/driver.
- **Apple Continuity / Nearby BLE payload fields.** Nominally rotate with
  the RPA; historically several fields did not rotate in lockstep, which
  permits cross-rotation linking.
- **Legacy leaks worth opportunistically recording:** WPS UUID-E (derived
  from the real MAC), directed probes carrying the preferred-network list,
  hotspot / Wi-Fi Direct SSIDs containing device names.
- **RF-layer fingerprints** (carrier frequency offset, IQ imbalance,
  transient). Immune to every software mitigation, but needs IQ samples,
  not frames from a managed-mode NIC. Out of scope unless the RTL-SDR path
  grows a Wi-Fi capability.

Open question: where do these live? Adding them to `emission.payload`
keeps the "payload is raw capture" invariant honest (they *are* raw capture
data), but grows the JSONB on the highest-volume table. Worth measuring
before committing.

## Rec 4 — clustering pass

Group ephemeral-class observations into a device cluster; have co-travel
score clusters rather than raw emitters.

- Shape it like the existing 60-second Standalone localization pass — that
  gives a working precedent for a periodic background analysis job.
- Cluster key: IE fingerprint + sequence-number continuity + spatial /
  temporal proximity. RSSI-and-GPS continuity across a MAC change is a good
  prior, and the localization work already computes those inputs.
- New `device_cluster` table; emissions/emitters get a nullable
  `cluster_id`. Co-travel's aggregate query groups on
  `COALESCE(cluster_id, emitter_id)` so non-randomized emitters keep
  working unchanged.
- Clustering must be revisable — a merge decision made on 3 observations
  should be re-evaluated when there are 30. Store the evidence, not just
  the verdict.
- Watch the false-merge direction carefully. Merging two strangers into one
  cluster manufactures a high-spread, long-span track, which is exactly the
  shape that scores Critical. For a stalking detector a false Critical is
  worse than a miss: it burns the operator's trust in the alert.

## Sequencing

3 before 4 (4 has nothing to cluster on without 3). 3 should land sooner
rather than later regardless of when 4 does, because it is the only part
that loses value by waiting.

---

## Related, not yet scheduled

Recommendation 2 from the same review — **guard identity values** — is
still open and is much smaller than either of the above.
`non_empty_str` (`classify.rs`) filters `""` but not all-zero or broadcast
addresses, and BLE identity-on-name accepts names of any length. Both
produce false merges that feed the co-travel score:

- `bluetooth_device:00:00:00:00:00:00` exists in the capture — 23
  emissions, 2.4 km spread, multiple distinct devices merged into one
  emitter.
- `bluetooth_device:OB` — a two-character name, 4 hits, 3.1 km spread.
  Generic names (`AirPods`, a car model) merge strangers the same way.

Fix is ~10 lines: reject all-zero/broadcast addresses and names below ~3
characters as `identity_value`, falling back to the address. Listed here so
it isn't lost, but it's independent of the clustering work above.

### Why the OR match rule doesn't already cover this

A reasonable objection: random-and-named BLE emitters get
`match_criteria = ANY[name == n, address == a]`, so doesn't the address
condition anchor the identity? No — three reasons, all confirmed against
the capture:

1. **`ANY`, not `ALL`.** Either condition alone matches, so
   `name == "4"` attributes any advertisement named "4" whatever its
   address.
2. **The address is frozen at creation.** `bluetooth_device:OB`'s stored
   rule pins exactly one address (`ff:f7:62:8e:ed:ad`), the one seen when
   the emitter was created. It never accumulates the others. The address
   arm matches one address ever; the name arm does all the work.
3. **The name arm is unbounded** — nothing requires the name to be
   distinctive.

Evidence that this produces real false merges, not just wide RPA
collapsing: `bluetooth_device:4` (a one-character name) holds 69 emissions
across **18 distinct addresses**, and 8 of those address pairs *overlap in
time* — e.g. `6f:23:c9:c7:c3:43` (19:21:39–19:21:49) and
`74:42:36:9c:8b:f7` (19:21:43–19:22:24) were advertising simultaneously.
One device cannot do that, so these are distinct devices merged into one
emitter with a fabricated multi-kilometre track.

Five name-keyed emitters span multiple addresses in the sample.

Note the design is *correct* for its purpose — a rotating RPA re-advertising
a distinctive name (`Dean's iPhone`) should collapse onto one emitter, and
it does. The defect is applying that same trust to a one-character name. A
length guard is the cheap fix; the principled version is a name-stability
check (does this name ever appear at two addresses simultaneously?), which
is really a special case of the clustering work above.
