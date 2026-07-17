# FluxFang

Self-hosted signals-intelligence platform. It captures RF emissions (802.11
WiFi frames) and GPS fixes on a Linux host, classifies them, and lets you
track emitters, entities, zones, and alerts over time — all through a web UI.

Runs fully on a built-in mock capturer, so you can bring the whole stack up
with **no RF/GPS hardware** attached.

## Requirements

- A **Linux host** (WiFi monitor-mode capture is Linux-specific).
- **Docker + Docker Compose v2** (`docker compose ...`).
- *Optional:* a monitor-mode WiFi adapter and/or a GPS receiver for real
  capture. Add these later from the web UI once the stack is running.

## Quick start

```bash
# 1. Create your environment file from the template
cp env.example .env

# 2. Edit .env and set your secrets (see below), then bring the stack up
docker compose up -d --build
```

In `.env` you must set at least:

- `POSTGRES_PASSWORD` — and update `DATABASE_URL` to match it.
- `FLUXFANG_SECRET_KEY` — generate a fresh one (must decode to 32 bytes):
  ```bash
  openssl rand -base64 32
  ```

Every other value in `env.example` has sane defaults and inline comments —
leave hardware settings (`WIFI_DEVICE`, `GPS_DEVICE`) unset if you have none.

Once the containers are up, open **`http://<host>:8081`** and complete the
first-run setup to choose your admin password.

## Managing the stack

```bash
docker compose ps                 # status of all services
docker compose logs -f            # follow logs (add a service name to narrow)
docker compose up -d --build      # (re)build and start in the background
docker compose restart backend    # restart a single service
docker compose down               # stop and remove containers
docker compose down -v            # also wipe the database volume (destructive)
```

Services: `db` (PostgreSQL + PostGIS), `backend` (Rust API), `frontend`
(React UI served by nginx).

> **Note:** the `backend` service runs `privileged` with host networking so it
> can access physical RF/GPS hardware. Treat it as host-root-equivalent — don't
> expose it to untrusted operators.
