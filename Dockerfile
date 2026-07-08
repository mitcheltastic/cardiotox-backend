# Builder stage
FROM rust:1-slim AS builder

WORKDIR /app

# Install ca-certificates (though rustls usually doesn't need pkg-config or libssl-dev, ca-certs is good to have)
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Dependency caching layer
# Create a dummy src/main.rs to build dependencies
RUN mkdir src && echo "fn main() {}" > src/main.rs
COPY Cargo.toml Cargo.lock ./
RUN cargo build --release
RUN rm -rf src

# Copy the rest of the source
COPY src ./src
COPY migrations ./migrations

# Build the real binary (we touch main.rs to ensure it's rebuilt)
RUN touch src/main.rs && cargo build --release

# Runtime stage
FROM debian:bookworm-slim

# Install ca-certificates for outbound HTTPS
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Run as non-root user
RUN useradd -m appuser
USER appuser

# Copy the compiled binary and migrations from the builder
COPY --from=builder /app/target/release/cardiotox-backend /app/
COPY --from=builder /app/migrations /app/migrations

# Expose port (default 3000, Render injects $PORT)
EXPOSE 3000

# Run the app
CMD ["/app/cardiotox-backend"]
