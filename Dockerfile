FROM node:24-bookworm-slim AS frontend-build

WORKDIR /build/frontend
COPY frontend/package.json frontend/package-lock.json ./
RUN npm ci
COPY frontend/ ./
RUN npm run build

FROM rust:1-bookworm AS backend-build

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/
RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=backend-build /build/target/release/hxx-web /usr/local/bin/hxx-web
COPY --from=frontend-build /build/frontend/dist ./frontend/dist

ENV HXX_BIND=0.0.0.0:3000
EXPOSE 3000
CMD ["hxx-web"]
