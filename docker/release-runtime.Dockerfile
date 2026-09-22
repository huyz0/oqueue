# M13.12: package an already-built release binary without rebuilding source.
FROM debian:bookworm-slim

COPY oqueue /usr/local/bin/oqueue
RUN chmod 0555 /usr/local/bin/oqueue

ENTRYPOINT ["/usr/local/bin/oqueue"]
