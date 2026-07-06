# Multi-stage build: compile a release binary, then ship it on a tiny runtime.
# Build:  docker build -t agentskillpack .
# Run:    docker run --rm -v "$PWD:/work" -w /work agentskillpack verify s.skillpack

# ---- build stage ----
FROM rust:1.83-slim AS build
WORKDIR /src

# Cache dependencies: copy manifests, fetch, then copy sources.
COPY Cargo.toml Cargo.lock ./
# A stub main so `cargo fetch` has something coherent to resolve against.
RUN mkdir -p src && echo "fn main() {}" > src/main.rs && \
    echo "" > src/lib.rs && \
    cargo fetch

# Real sources.
COPY src ./src
COPY examples ./examples
COPY docs ./docs
COPY tests ./tests
RUN cargo build --release --locked && \
    strip target/release/agentskillpack || true

# ---- runtime stage ----
FROM debian:bookworm-slim AS runtime
LABEL org.opencontainers.image.title="agentskillpack" \
      org.opencontainers.image.description="Portable, signed, capability-declaring packaging format and registry for AI-agent skills." \
      org.opencontainers.image.source="https://github.com/cognis-digital/agentskillpack" \
      org.opencontainers.image.licenses="LicenseRef-COCL-1.0"

# Non-root user.
RUN useradd --create-home --uid 10001 skilluser
COPY --from=build /src/target/release/agentskillpack /usr/local/bin/agentskillpack
USER skilluser
WORKDIR /work
ENTRYPOINT ["agentskillpack"]
CMD ["--help"]
