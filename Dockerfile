# syntax=docker/dockerfile:1.7
#
# Runtime image. The workflow copies one static musl Linux executable
# into this context as ./monoize (CI-B4 through CI-B6).

FROM ubuntu:24.04@sha256:33ceb71981b602c1a7443a53469e4dba065f7503eab3078a2d7a57a2ab987517

ARG VERSION=dev
ARG REVISION=unknown

LABEL org.opencontainers.image.title="Monoize" \
      org.opencontainers.image.description="Self-hosted AI API gateway with protocol conversion and multi-provider routing" \
      org.opencontainers.image.source="https://github.com/Ikaleio/monoize" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.revision="${REVISION}"

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --user-group --home-dir /app --shell /usr/sbin/nologin monoize \
    && install --directory --owner monoize --group monoize /app/data

COPY --chown=monoize:monoize --chmod=0755 monoize /usr/local/bin/monoize

USER monoize
WORKDIR /app

VOLUME ["/app/data"]
EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl --fail --silent --show-error http://127.0.0.1:8080/ >/dev/null || exit 1

ENTRYPOINT ["monoize"]
