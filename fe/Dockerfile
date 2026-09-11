# The React app, built to static files and served by nginx. Build context is the repo root: the
# app depends on `@solvent/sdk` as a workspace package, so the SDK must be built first.
FROM node:22-slim AS build
RUN corepack enable
WORKDIR /src
COPY . .
RUN pnpm install --frozen-lockfile

# Vite inlines `VITE_*` at BUILD time, so the API origin is baked into the bundle and cannot be
# changed by a runtime env var. Rebuild the image to point the app at a different backend.
ARG VITE_API_BASE_URL=http://127.0.0.1:8099
ENV VITE_API_BASE_URL=$VITE_API_BASE_URL
RUN pnpm --filter @solvent/sdk build \
    && pnpm --filter @solvent/fe build

FROM nginx:alpine
COPY --from=build /src/fe/dist /usr/share/nginx/html
# A single-page app: unknown paths are client-side routes, not 404s.
RUN printf 'server {\n\
  listen 80;\n\
  root /usr/share/nginx/html;\n\
  location / { try_files $uri $uri/ /index.html; }\n\
}\n' > /etc/nginx/conf.d/default.conf
EXPOSE 80
HEALTHCHECK --interval=15s --timeout=3s --retries=3 \
    CMD wget -qO- http://127.0.0.1/ >/dev/null || exit 1
