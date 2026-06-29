# FROM backpackapp/build:v0.31.0

# WORKDIR /app

# COPY . .


# COPY config/id-test.json /app/target/deploy/id.json
# ENV ANCHOR_WALLET=/app/target/deploy/id.json

# RUN npm ci
# RUN anchor build

# EXPOSE 8899 8900

# COPY scripts/start-solana.sh /start.sh
# RUN chmod +x /start.sh

# CMD ["/start.sh"]


FROM debian:trixie-slim

RUN apt-get update && apt-get install -y \
	curl build-essential pkg-config libssl-dev libudev-dev clang libclang-dev \
	bzip2 ca-certificates git

# Rust 1.91.0 (>= 1.85 required for Anchor 1.0.2 / edition2024)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.91.0
ENV PATH="/root/.cargo/bin:${PATH}"

# Solana CLI 2.3.9 (no io_uring requirement)
RUN curl -sSfL https://github.com/anza-xyz/agave/releases/download/v2.3.9/solana-release-x86_64-unknown-linux-gnu.tar.bz2 | tar xj -C /opt
ENV PATH="/opt/solana-release/bin:${PATH}"

# Install AVM and Anchor 1.0.2
RUN cargo install --git https://github.com/coral-xyz/anchor avm --force
RUN avm install 1.0.2 && avm use 1.0.2

# Node 20 (18 is EOL; Anchor docs use 20+)
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && \
	apt-get install -y nodejs

WORKDIR /app

# Cache deps first
COPY Cargo.toml Cargo.lock* ./
COPY programs/ ./programs/
COPY package.json package-lock.json ./
RUN npm ci

# Now the rest
COPY . .

RUN mkdir -p /root/.config/solana
COPY ./config/id-test.json /root/.config/solana/id-test.json
ENV ANCHOR_WALLET=/root/.config/solana/id-test.json

RUN anchor build

EXPOSE 8899 8900

COPY scripts/start-solana.sh /start.sh
RUN chmod +x /start.sh

CMD ["/start.sh"]