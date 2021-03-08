FROM rust:1.50

WORKDIR /usr/src/app

COPY . .

RUN rustup toolchain install nightly && \
    rustup default nightly

RUN cargo build --release

CMD ["./target/release/gotchi"]