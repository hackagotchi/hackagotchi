FROM rust:1.50 as builder

WORKDIR /usr/src/app

COPY . .

RUN rustup toolchain install nightly && \
    rustup default nightly

RUN cargo build --release

FROM debian:buster as runner

WORKDIR /usr/src/app

COPY --from=builder /usr/src/app/target/release/gotchi ./gotchi

CMD [ "./gotchi" ]