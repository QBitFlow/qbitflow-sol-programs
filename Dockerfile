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


FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
	curl build-essential pkg-config libssl-dev libudev-dev clang libclang-dev \
	bzip2 ca-certificates

# Install exact Rust version
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.89.0
ENV PATH="/root/.cargo/bin:${PATH}"

 # Install Solana CLI (prebuilt binary)
RUN curl -sSfL https://github.com/anza-xyz/agave/releases/download/v2.3.9/solana-release-x86_64-unknown-linux-gnu.tar.bz2 | tar xj -C /opt
ENV PATH="/opt/solana-release/bin:${PATH}"

# Install Anchor CLI
RUN cargo install --git https://github.com/coral-xyz/anchor --tag v0.31.1 anchor-cli

# Install Node
RUN curl -fsSL https://deb.nodesource.com/setup_18.x | bash - && \
	apt-get install -y nodejs

WORKDIR /app
COPY . .

RUN mkdir -p /root/.config/solana
COPY ./config/id-test.json /root/.config/solana/id-test.json
ENV ANCHOR_WALLET=/root/.config/solana/id-test.json


RUN npm ci
RUN anchor build

EXPOSE 8899 8900

COPY scripts/start-solana.sh /start.sh
RUN chmod +x /start.sh

CMD ["/start.sh"]