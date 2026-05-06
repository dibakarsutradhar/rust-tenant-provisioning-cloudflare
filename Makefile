# Makefile
.PHONY: help setup dev stop reset migrate tunnel logs db

# default target
help:
	@echo ""
	@echo "  make setup     — first time setup (copy .env, start docker, migrate)"
	@echo "  make dev       — start the app (docker + cargo run)"
	@echo "  make stop      — stop docker containers"
	@echo "  make reset     — wipe DB and start fresh"
	@echo "  make migrate   — run pending migrations"
	@echo "  make tunnel    — start cloudflare tunnel"
	@echo "  make logs      — tail docker logs"
	@echo "  make db        — open psql"
	@echo ""

# ── first time setup ─────────────────────────────────────────────────────────
setup:
	@echo "→ Copying .env.example to .env (skip if exists)"
	@test -f .env || cp .env.example .env
	@echo "→ Starting Docker services"
	docker compose up -d
	@echo "→ Waiting for Postgres to be ready"
	@until docker compose exec postgres pg_isready -U garageos > /dev/null 2>&1; do sleep 1; done
	@echo "→ Running migrations"
	sqlx migrate run
	@echo ""
	@echo "✓ Setup complete. Edit .env with your values then run: make dev"

# ── start everything ──────────────────────────────────────────────────────────
dev:
	@echo "→ Starting Docker services"
	docker compose up -d
	@echo "→ Starting Rust app"
	RUST_LOG=$${RUST_LOG:-debug} cargo run

# ── tunnel (separate terminal) ────────────────────────────────────────────────
tunnel:
	cloudflared tunnel --config cloudflared/config.yml run

# ── stop docker ───────────────────────────────────────────────────────────────
stop:
	docker compose stop

# ── run migrations ────────────────────────────────────────────────────────────
migrate:
	sqlx migrate run

# ── wipe and rebuild DB ───────────────────────────────────────────────────────
reset:
	@echo "→ Dropping and recreating database"
	docker compose exec postgres psql -U garageos -c "DROP DATABASE IF EXISTS garageos;"
	docker compose exec postgres psql -U garageos -c "CREATE DATABASE garageos;"
	@echo "→ Running migrations"
	sqlx migrate run
	@echo "✓ Database reset complete"

# ── tail logs ─────────────────────────────────────────────────────────────────
logs:
	docker compose logs -f

# ── open psql ─────────────────────────────────────────────────────────────────
db:
	psql $${DATABASE_URL:-postgresql://garageos:secret@localhost:5432/garageos}

# ── build release ─────────────────────────────────────────────────────────────
build:
	cargo build --release

# ── check everything compiles ─────────────────────────────────────────────────
check:
	cargo check

# ── generate cloudflared config from template ─────────────────────────────────
tunnel-config:
	@echo "→ Detecting local IP..."
	@echo "  IP: $(LOCAL_IP)"
	@test -n "$(LOCAL_IP)" || (echo "ERROR: could not detect local IP" && exit 1)
	@source .env && sed \
		-e "s|{{CLOUDFLARE_TUNNEL_NAME}}|$$CLOUDFLARE_TUNNEL_NAME|g" \
		-e "s|{{CLOUDFLARE_TUNNEL_ID}}|$$CLOUDFLARE_TUNNEL_ID|g" \
		-e "s|{{HOME}}|$$HOME|g" \
		-e "s|{{BASE_DOMAIN}}|$$BASE_DOMAIN|g" \
		-e "s|{{APP_SUBDOMAIN}}|$$APP_SUBDOMAIN|g" \
		-e "s|{{LOCAL_IP}}|$(LOCAL_IP)|g" \
		-e "s|{{NGINX_PORT}}|$${NGINX_PORT:-80}|g" \
		cloudflared/config.yml.template > cloudflared/config.yml
	@echo "✓ Generated cloudflared/config.yml"

# ── auto-detect local IP and generate ────────────────────────────────────────
tunnel-config-auto:
	@echo "→ Auto-detecting local IP"
	@LOCAL_IP=$$(ipconfig getifaddr en0 || ip route get 1 | awk '{print $$7; exit}') && \
	echo "  Local IP: $$LOCAL_IP" && \
	source .env && sed \
		-e "s|{{CLOUDFLARE_TUNNEL_NAME}}|$$CLOUDFLARE_TUNNEL_NAME|g" \
		-e "s|{{CLOUDFLARE_TUNNEL_ID}}|$$CLOUDFLARE_TUNNEL_ID|g" \
		-e "s|{{HOME}}|$$HOME|g" \
		-e "s|{{BASE_DOMAIN}}|$$BASE_DOMAIN|g" \
		-e "s|{{APP_SUBDOMAIN}}|$$APP_SUBDOMAIN|g" \
		-e "s|{{LOCAL_IP}}|$$LOCAL_IP|g" \
		-e "s|{{NGINX_PORT}}|$${NGINX_PORT:-80}|g" \
		cloudflared/config.yml.template > cloudflared/config.yml
	@echo "✓ cloudflared/config.yml generated"
	@cat cloudflared/config.yml

# update tunnel target to generate config first
tunnel: tunnel-config-auto
	cloudflared tunnel --config cloudflared/config.yml run