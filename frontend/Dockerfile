# --- Build stage ---
FROM node:20 AS builder

WORKDIR /app
COPY package*.json ./
# This repo's package-lock.json was generated with a newer npm (>=11) than the
# one bundled in the node:20 image (10.8.2), and `npm ci`'s strict lockfile
# check rejects it as a result. Upgrade npm in the build stage first so `npm
# ci` can run against the committed lockfile for a reproducible install.
#
# The npm version is pinned to an exact release (not a floating `npm@11`):
# testing showed npm 11.6.2 installs cleanly against this lockfile, while
# every 11.x release from 11.7.0 onward (up to 11.18.0 at the time of
# writing) fails `npm ci` with a spurious "Missing: @emnapi/core@1.11.2 from
# lock file" error caused by a change in npm's optional-dependency
# resolution. Floating to `npm@11` would therefore reintroduce the exact
# non-reproducibility this fix is meant to solve, since it silently picks up
# whatever the latest 11.x patch is on the day of the build.
RUN npm install -g npm@11.6.2 && npm ci
COPY . .
RUN npm run build

# --- Runtime stage ---
FROM nginx:alpine

COPY --from=builder /app/dist /usr/share/nginx/html
COPY nginx.conf /etc/nginx/conf.d/default.conf

EXPOSE 8081
