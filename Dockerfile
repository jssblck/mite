#
# Usage:
#
# - server: build with `docker build -t $TAG --target server .`
#
# Details:
#
# - Builds binaries incrementally with multi-stage builds to improve caching
# - Builds a totally distinct runtime image with only the things required at runtime
# - Copies the built binaries from base to runtime as the final step

FROM debian:bookworm-slim AS os
WORKDIR /app
RUN apt-get update && \
    apt-get -y install build-essential && \
    rm -rf /var/lib/apt/lists/*
RUN curl https://sh.rustup.rs -sSf | sh -s -- --default-toolchain stable -y
RUN curl https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh -sSf | bash
ENV PATH="/root/.cargo/bin:$PATH"
RUN cargo binstall cargo-chef

FROM os AS planner
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM planner AS builder
ARG BUILD_VERSION=unknown
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim AS runtime-base
RUN adduser --uid 1000 mite
WORKDIR /home/sparkle
USER mite

# If no target is specified, the last target in the file is built;
# the server is the most useful to build in this context.
FROM runtime-base AS server
COPY --from=builder /app/target/release/mite /usr/local/bin
ENTRYPOINT ["/usr/local/bin/mite"]
