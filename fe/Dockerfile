# Build context is the workspace root: fe depends on the local @solvent/sdk workspace package, so
# the whole pnpm workspace must be present to resolve it. Only the final static build output
# (fe/dist) crosses into the runtime layer.
FROM node:22-slim AS build
RUN corepack enable && corepack prepare pnpm@10.17.1 --activate
WORKDIR /src
COPY . .
RUN pnpm install --frozen-lockfile
RUN pnpm --filter @solvent/sdk run build
ARG VITE_API_BASE_URL
ARG VITE_FAUCET_BASE_URL
ARG VITE_PRIVY_APP_ID
ARG VITE_CHAIN_ID
ARG VITE_CHAIN_NAME
ARG VITE_RPC_URL
ARG VITE_DESTINATION_CHAIN_ID
ARG VITE_DESTINATION_CHAIN_NAME
ARG VITE_DESTINATION_RPC_URL
ARG VITE_SOLVENTX_BASE_API_URL
ARG VITE_SOLVENTX_ORIGIN_API_URL
ARG VITE_SOLVENTX_PROXY_URL
RUN pnpm --filter @solvent/fe run build

FROM nginx:alpine
COPY --from=build /src/fe/dist /usr/share/nginx/html
EXPOSE 80
