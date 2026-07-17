# FluxFang

FluxFang is a self-hosted signals-intelligence platform. It listens for RF
emissions (802.11 WiFi frames, Bluetooth devices, and TPMS tire-pressure
sensor transmissions) along with GPS fixes on a Linux host, classifies what it
hears, and lets you track emitters, entities, zones, and alerts over time. You
drive the whole thing from a web UI.

## Requirements

- A **Linux host**. WiFi monitor-mode capture is Linux-specific.
- **Docker** and **Docker Compose v2** (the `docker compose ...` command).
- Optionally, any mix of the hardware below for real capture. You add each one
  later from the web UI once the stack is running:
  - a monitor-mode-capable **WiFi adapter** for 802.11 capture
  - an **RTL-SDR dongle** for TPMS capture
  - a **Bluetooth adapter** for Bluetooth capture
  - a **GPS receiver** for geotagging, either serial/USB or a networked `gpsd`

## Quick start

```bash
# 1. Clone the repo and move into it
git clone https://github.com/reconhawklabs/FluxFang.git
cd FluxFang

# 2. Create your environment file from the template
cp env.example .env

# 3. Edit .env and set your secrets, then bring the stack up
docker compose up -d --build
```

Every value in `env.example` ships with a sane default and an inline comment
explaining it. If you have no hardware yet, just leave the hardware settings
(`WIFI_DEVICE`, `GPS_DEVICE`) unset.

Once the containers are up, open **`http://<host>:8081`**, complete the
first-run setup, and choose your admin password.

## Managing the stack

```bash
docker compose ps                 # status of all services
docker compose logs -f            # follow logs (add a service name to narrow)
docker compose up -d --build      # (re)build and start in the background
docker compose restart backend    # restart a single service
docker compose down               # stop and remove containers
docker compose down -v            # also wipe the database volume (destructive)
```

The stack has three services: `db` (PostgreSQL with PostGIS), `backend` (the
Rust API), and `frontend` (the React UI served by nginx).

> **Note:** the `backend` service runs `privileged` with host networking so it
> can reach physical RF and GPS hardware. Treat it as host-root-equivalent, and
> don't expose it to untrusted operators.

## Running on Windows (WSL2)

The web stack runs fine under WSL2 (Ubuntu): install Docker and follow the Quick
start above, then open `http://localhost:8081` from Windows.

RF capture is the catch. WSL2 doesn't expose your PC's built-in radios to Linux.
USB devices (RTL-SDR, USB-serial GPS) can be passed through with
[usbipd-win](https://github.com/dorssel/usbipd-win), but monitor-mode WiFi needs
a custom WSL2 kernel and generally won't work out of the box. For real capture,
use a native Linux host.
