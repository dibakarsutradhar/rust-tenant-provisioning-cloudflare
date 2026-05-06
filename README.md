git clone <repo>
cd <repo>

# 1. install deps (one time)
brew install cloudflared sqlx-cli

# 2. setup
make setup

# 3. edit .env with your CF credentials
nano .env

# 4. terminal 1 — run app
make dev

# 5. terminal 2 — run tunnel
make tunnel