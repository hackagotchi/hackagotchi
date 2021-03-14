FROM rust:1.50

WORKDIR /usr/src/app

COPY . .

RUN rustup toolchain install nightly && \
    rustup default nightly

RUN cargo build --release

EXPOSE 80

CMD ["./target/release/gotchi"]