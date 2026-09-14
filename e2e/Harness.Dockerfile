FROM node:24-bookworm-slim@sha256:ba849c60be29959425b8734d57b8b4b7d56f98edd9504c9af091d5281095a71e AS bun
RUN npm install --global bun@1.4.2
FROM node:24-bookworm-slim@sha256:ba849c60be29959425b8734d57b8b4b7d56f98edd9504c9af091d5281095a71e
COPY --from=bun /usr/local/lib/node_modules/bun/bin/bun.exe /usr/local/bin/bun
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates git ripgrep && rm -rf /var/lib/apt/lists/* && usermod -l harness -d /home/harness node && mkdir -p /home/harness /work && chown -R 1000:1000 /home/harness /work
WORKDIR /suite
COPY package.json bun.lock ./
RUN --mount=type=cache,target=/root/.bun/install/cache bun install --frozen-lockfile
COPY *.ts *.js ./
ENV PATH="/suite/node_modules/.bin:/usr/local/bin:/usr/bin:/bin"
RUN codex --version && claude --version && opencode --version && pi --version
ENTRYPOINT ["bun"]
CMD ["/suite/worker.ts"]
