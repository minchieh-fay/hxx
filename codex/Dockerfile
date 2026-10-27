FROM debian:bookworm-slim

ARG DEBIAN_FRONTEND=noninteractive
ARG KUBECTL_VERSION=stable
ARG NODE_VERSION=22.23.4
ARG BUN_VERSION=1.4.4

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        build-essential \
        curl \
        dnsutils \
        git \
        iproute2 \
        jq \
        netcat-openbsd \
        openssh-client \
        python-is-python3 \
        python3 \
        python3-pip \
        python3-venv \
        ripgrep \
        sshpass \
        tcpdump \
        tini \
        xz-utils \
    && rm -rf /var/lib/apt/lists/*

RUN set -eu; \
    case "$(dpkg --print-architecture)" in \
        amd64) node_arch=x64 ;; \
        arm64) node_arch=arm64 ;; \
        *) echo "Unsupported architecture: $(dpkg --print-architecture)" >&2; exit 1 ;; \
    esac; \
    curl -fsSLo /tmp/node.tar.xz \
        "https://nodejs.org/dist/v${NODE_VERSION}/node-v${NODE_VERSION}-linux-${node_arch}.tar.xz"; \
    tar -xJf /tmp/node.tar.xz --strip-components=1 -C /usr/local; \
    ln -sf /usr/local/bin/node /usr/local/bin/nodejs; \
    rm /tmp/node.tar.xz

RUN npm install --global "bun@${BUN_VERSION}" \
    && npm cache clean --force \
    && node --version \
    && npm --version \
    && bun --version \
    && python3 --version \
    && sshpass -V \
    && tcpdump --version

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
