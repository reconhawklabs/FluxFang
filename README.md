# FluxFang

FluxFang is a self-hosted signals-intelligence platform for a Linux host with
an attached WiFi adapter (and optionally a GPS receiver). It captures RF
emissions (currently 802.11 WiFi management/probe frames) and GPS fixes,
classifies them, and lets you build a picture of what's around you over time:

- **Emissions** — raw sightings (a beacon/probe frame seen at a point in time,
  optionally geotagged with the host's current GPS fix).
- **Emitters** — a persistent "thing" (identified by a rule, e.g. "bssid =
  aa:bb:cc:dd:ee:ff") that groups many emissions together.
- **Entities** — a human-meaningful label ("Bob's phone", "Neighbor's AP")
  associated with one or more emitters.
- **Zones** — named geofences; emitters/entities/the host itself trigger
  "enters"/"leaves" events as they cross a zone boundary.
- **Alerts** — rules ("notify me when X is detected" / "notify me when X
  enters/leaves zone Y") wired to alert methods (email, webhook, in-app),
  producing a Notifications inbox.

Everything above runs whether or not real hardware is attached — a
deterministic mock capturer stands in for the WiFi/GPS hardware so the whole
pipeline (ingest → emitters → zones → alerts → notifications) can be
exercised in development or a demo without any dongles.

## Architecture

- **Backend** — Rust (`backend/crates/`): `fluxfang-api` (Axum HTTP + WebSocket
  server, REST endpoints, session auth), `fluxfang-capture` (WiFi
  monitor-mode frame capture via `iw`+`pcap`, GPS via `gpsd` or a serial NMEA
  device), `fluxfang-core` (pure rule/condition engine used both to auto-attach
  emissions to emitters and to evaluate alert rules), `fluxfang-db` (sqlx
  repositories over Postgres/PostGIS).
- **Database** — PostgreSQL + PostGIS (geography columns for emission/zone
  locations, `ST_DWithin` for zone membership).
- **Frontend** — React + TypeScript (`frontend/`), TanStack Query, MapLibre GL
  for the map view, served by nginx and proxying `/api` and `/ws` to the
  backend.
- **Realtime** — a `/ws` WebSocket streams live `emission`/`notification`
  events to the frontend as they're ingested.
- **Deployment** — `docker-compose.yml` wires all three services together
  (`db`, `backend`, `frontend`).

## Host prerequisites

- **Linux host.** WiFi monitor-mode capture and raw-socket access are
  Linux-specific (`iw`, `NET_ADMIN`/`NET_RAW`, `network_mode: host`).
- **A monitor-mode-capable WiFi adapter** and the `iw` tool (already baked
  into the backend's container image) if you want real WiFi capture. Not
  required to run the stack — see "no hardware" below.
- **Optional GPS**, via either:
  - a **`gpsd` server** reachable at some `host:port` (can be on the same
    host or elsewhere on the network — no special container wiring needed,
    the backend just connects to it), or
  - a **serial/USB GPS receiver** (a `/dev/ttyUSB0`-style NMEA device) plus
    a baud rate.
- **Docker + Docker Compose v2** (`docker compose ...`, not the standalone
  `docker-compose` v1 binary).
- **No hardware at all works too.** Every data source can instead be created
  as a `mock` source (see Development, below) so you can run the full stack,
  UI included, without any RF/GPS hardware.

### The privileged-container caveat

The `backend` service in `docker-compose.yml` runs with `network_mode: host`,
`privileged: true`, and `cap_add: [NET_ADMIN, NET_RAW]`. This is required
because:

- Putting a WiFi adapter into monitor mode and reading raw 802.11 frames off
  it needs `NET_ADMIN`/`NET_RAW` against a **host** network interface — a
  container-private network namespace (the Docker default) wouldn't even see
  the physical adapter.
- `network_mode: host` also means the backend can reach `db` at
  `127.0.0.1:5432` and the frontend can reach the backend at
  `127.0.0.1:8080`/`8080/ws` without relying on Docker's bridge DNS.

**Security implication:** a `privileged` container with host networking has
no meaningful isolation from the host — it can see and manipulate all host
network interfaces and (with `privileged: true`) effectively all host
devices. This is an intentional, accepted trade-off for a tool whose entire
purpose is to own physical RF/GPS hardware, but it means you should treat the
backend container as having host-root-equivalent capability. Don't expose it
to untrusted operators, and keep the host itself patched and firewalled as
you would for any process running with this level of access.

## Setup / running

```bash
git clone <this repo>
cd FluxFang
cp .env.example .env
```

Edit `.env`:

- Set `POSTGRES_PASSWORD` (and update `DATABASE_URL` to match).
- Generate a real `FLUXFANG_SECRET_KEY` (must decode to exactly 32 bytes —
  the backend fails fast at startup on anything else, since it's the
  AES-256-GCM key used to encrypt alert-method credentials at rest):
  ```bash
  openssl rand -base64 32
  ```
- `FLUXFANG_SESSION_KEY` can be left as-is for now (reserved for a future
  persistent/signed session store; sessions today are server-side opaque IDs
  with no client-editable payload to sign).
- Leave `WIFI_DEVICE`/`GPS_DEVICE` unset if you have no hardware yet, or set
  them per the comments in `.env.example` (see "Hardware passthrough notes"
  below).

Then bring the stack up:

```bash
docker compose up --build
```

Open the frontend at `http://<host>:8081` (nginx's exposed port on
`network_mode: host`). On first load you'll be prompted to complete
**first-run setup** (`POST /api/setup`) — this sets the single admin WebUI
password. After that, log in with that password.

## Using it

A brief walk-through of the main flow once you're logged in:

1. **Data Sources** — add a source:
   - *WiFi*: pick `wifi` / mode `monitor`, and give the **interface name**
     of a monitor-capable adapter (e.g. `wlan1`). This is the same string as
     `WIFI_DEVICE`, just entered per-source rather than baked into the
     container.
   - *GPS*: pick `gps`, then either mode `gpsd` (`{host, port}` of a running
     `gpsd`) or mode `serial` (`{device, baud}`, e.g. `/dev/ttyUSB0` at
     `4800`/`9600`/`115200` — see `fluxfang_capture::gps::ALLOWED_BAUD_RATES`
     for the exact allow-list).
   - *Mock* (no hardware): a synthetic source that emits deterministic fake
     WiFi/GPS traffic — good for trying out the rest of the app end to end.
   - Click **Start** on the source. Status flips `starting` → `running` (or
     `error` with `last_error` explaining what went wrong, e.g. a bad
     interface name).
2. **Emissions** — browse/filter what's been captured (kind, payload fields
   like bssid/ssid/channel, time range, location). Select rows and **assign
   to an emitter** to start grouping them.
3. **Emitters** — each emitter has a match rule (built with the same
   catalog-driven rule builder used for alerts) that auto-attaches future
   matching emissions. Associate an emitter with an **Entity** (create a new
   one, or attach to an existing one) to give it a human label.
4. **Zones** — draw a named circular geofence (center + radius). Entities,
   emitters, and the host itself generate "entered zone"/"left zone" events
   as they cross it.
5. **Alerts** — add an **Alert Method** (email, webhook, or in-app) and one or
   more **Alert Rules** (trigger on "detected" with an optional content
   filter, or on "enters zone"/"leaves zone" for a target entity), pointing at
   one or more methods. Use "Send test" on a method to confirm delivery
   before relying on it.
6. **Notifications** — the inbox where fired alerts land, with an unread
   count in the nav; also streamed live over `/ws` as they happen.

## Development

Backend tests need a running Postgres:

```bash
docker compose up -d db
cd backend
DATABASE_URL=postgres://postgres:changeme@127.0.0.1:5432/fluxfang cargo test
```

(use whatever `POSTGRES_PASSWORD`/`DATABASE_URL` you set in `.env`; tests
create/drop their own per-test Postgres schemas against that database, so
pointing at a scratch/dev DB is fine.)

Frontend tests:

```bash
cd frontend
npm install
npx vitest run
```

You do **not** need real WiFi/GPS hardware to develop or demo FluxFang: every
data source can be created with kind `mock`, which drives the exact same
ingest → emitter-attach → zone-tracking → alert-evaluation pipeline as real
hardware, just fed by an in-process deterministic generator
(`fluxfang_capture`'s `MockCapturer`/`MockGps`) instead of `iw`/pcap/serial.

## Hardware passthrough notes

See the comments in `docker-compose.yml` and `.env.example` for the full
reasoning; in short:

- `WIFI_DEVICE` is just an **interface name** (e.g. `wlan1`), not a `/dev`
  node — no `devices:` entry is needed for it. `network_mode: host` +
  `privileged: true` + `NET_ADMIN`/`NET_RAW` already give the backend
  container direct access to every host network interface by name.
- `GPS_DEVICE` matters only for a **serial/USB** GPS receiver. When set,
  `docker-compose.yml` passes it through to the backend container as a
  Compose `devices:` entry (the `--device` equivalent), e.g.
  `GPS_DEVICE=/dev/ttyUSB0`. If you use a network `gpsd` server instead, skip
  `GPS_DEVICE` entirely and just configure the data source with that server's
  `host`/`port` from the WebUI.
- `GPS_DEVICE` defaults to `/dev/null` (always present, harmless) so
  `docker compose up` still starts cleanly with no GPS hardware attached.

## Manual verification checklist

Steps to validate an actual deployment end to end (this mirrors the
automated backend E2E test, but exercised through the real UI/stack):

1. `cp .env.example .env`, fill in `POSTGRES_PASSWORD`/`DATABASE_URL` and a
   fresh `FLUXFANG_SECRET_KEY` (`openssl rand -base64 32`).
2. `docker compose up --build` — confirm all three containers report
   healthy/running (`docker compose ps`).
3. Open the frontend; complete first-run setup (choose an admin password);
   log in with it.
4. Add a Data Source:
   - With real hardware: `wifi`/`monitor` + your adapter's interface name
     (or `gps`/`gpsd`|`serial` per above).
   - Without hardware: a `mock` source.
5. Click **Start** on the source; confirm its status becomes `running` (no
   `last_error`).
6. Open **Emissions** and confirm new rows appear (live, via the `/ws` feed,
   without needing to refresh).
7. Select a few emissions and **assign to an emitter**; confirm the emitter
   shows up under **Emitters** with a non-zero attached count.
8. Associate that emitter with a new **Entity**; confirm it appears under
   **Entities** with a `last_seen` timestamp.
9. Create a **Zone** around a location you expect a located emission/fix to
   fall inside; confirm the entity/emitter/host shows up under that zone's
   "currently inside" list once a matching location comes in.
10. Add an **Alert Method** (e.g. `in_app`) and use **Send test** to confirm
    delivery works before relying on it.
11. Add an **Alert Rule** targeting the entity from step 8 (`on: detected`,
    or `on: enters_zone` against the zone from step 9), pointing at the
    method from step 10.
12. Trigger a matching emission/fix and confirm a **Notification** appears
    in the inbox (and via the live `/ws` badge) attributable to that rule.

## Known limitations / not yet built

- No RSSI-based localization or triangulation — locations come only from the
  host's own GPS fix at the time of capture, not from signal strength.
- No "follow"/stalker-detection scoring across sessions.
- Additional emission kinds beyond WiFi 802.11 (e.g. Bluetooth, cellular) are
  not implemented.
- Single admin user/session model — no multi-user accounts or RBAC.
- Sessions are in-memory (`tower-sessions` `MemoryStore`); restarting the
  backend logs everyone out.
