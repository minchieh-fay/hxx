FROM debian:bookworm-slim

ARG DEBIAN_FRONTEND=noninteractive
ARG KUBECTL_VERSION=stable

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
        git \
        jq \
        openssh-client \
        ripgrep \
        tini \
    && rm -rf /var/lib/apt/lists/*

RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
        amd64) kubectl_arch=amd64 ;; \
        arm64) kubectl_arch=arm64 ;; \
        *) echo "Unsupported architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    if [ "$KUBECTL_VERSION" = "stable" ]; then \
        kubectl_version="$(curl -fsSL https://dl.k8s.io/release/stable.txt)"; \
    else \
        kubectl_version="$KUBECTL_VERSION"; \
    fi; \
    curl -fsSLo /usr/local/bin/kubectl \
        "https://dl.k8s.io/release/${kubectl_version}/bin/linux/${kubectl_arch}/kubectl"; \
    chmod +x /usr/local/bin/kubectl

RUN mkdir -p /opt/codex /codex-home /workspaces \
    && useradd --create-home --uid 10001 --shell /bin/bash codex \
    && chown -R codex:codex /opt/codex /codex-home /workspaces

ENV CODEX_HOME=/codex-home
ENV PATH=/opt/codex:$PATH
WORKDIR /workspaces

USER codex
EXPOSE 4500

ENTRYPOINT ["/usr/bin/tini", "--", "/opt/codex/codex"]
CMD ["--dangerously-bypass-approvals-and-sandbox", \
    "-c", "check_for_update_on_startup=false", \
    "app-server", "--listen", "ws://0.0.0.0:4500", \
    "--ws-auth", "capability-token", \
    "--ws-token-file", "/run/secrets/codex-ws-token"]
