FROM node:24-bookworm-slim@sha256:ba849c60be29959425b8734d57b8b4b7d56f98edd9504c9af091d5281095a71e AS bun
RUN npm install --global bun@1.4.2
FROM rust:1.98.0-bookworm@sha256:82150a52ec202c1b14d7817e14516c392bb7f5cfebd88f1ed531cb37ebd39922 AS build
COPY --from=bun /usr/local/lib/node_modules/bun/bin/bun.exe /usr/local/bin/bun
COPY --from=bun /usr/local/bin/node /usr/local/bin/node
RUN apt-get update && apt-get install -y --no-install-recommends cmake clang libclang-dev pkg-config nasm && rm -rf /var/lib/apt/lists/*
WORKDIR /source
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/source/target cargo build --release --locked && cp target/release/monoize /usr/local/bin/monoize-e2e
FROM node:24-bookworm-slim@sha256:ba849c60be29959425b8734d57b8b4b7d56f98edd9504c9af091d5281095a71e AS monoize
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates libstdc++6 && rm -rf /var/lib/apt/lists/*
COPY --from=build /usr/local/bin/monoize-e2e /usr/local/bin/monoize
WORKDIR /data
ENTRYPOINT ["monoize"]
