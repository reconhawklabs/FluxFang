# --- Build stage ---
FROM node:20 AS builder

WORKDIR /app
COPY package*.json ./
# Note: `npm ci` is intentionally not used here — this repo's package-lock.json
# was generated with a newer npm (>=11) than the one bundled in the node:20
# image (10.8.2), and `npm ci`'s strict lockfile check rejects it as a result.
# `npm install` resolves the same dependency tree without that mismatch.
RUN npm install
COPY . .
RUN npm run build

# --- Runtime stage ---
FROM nginx:alpine

COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf

EXPOSE 8081
