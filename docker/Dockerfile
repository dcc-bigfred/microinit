# Distroless multiarch image for microinit supervise mode.
# Build context: repository root with prebuilt binaries at:
#   dist/microinit-linux-amd64
#   dist/microinit-linux-arm64
#
# Example:
#   docker buildx build --platform linux/amd64,linux/arm64 \
#     -f docker/Dockerfile -t ghcr.io/dcc-bigfred/microinit:main --push .

FROM gcr.io/distroless/static-debian12:nonroot

ARG TARGETARCH
COPY dist/microinit-linux-${TARGETARCH} /microinit

USER nonroot:nonroot
ENTRYPOINT ["/microinit"]
CMD ["supervise"]
