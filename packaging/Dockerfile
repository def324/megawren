# Minimal image around the static Linux binary. CI unpacks each static
# release archive into target/image/<arch>/ and builds with that context:
#   docker buildx build --platform linux/amd64,linux/arm64 -f packaging/Dockerfile target/image
FROM scratch
ARG TARGETARCH
COPY ${TARGETARCH}/ /
# The saved login and file index live in /data/.megawren. Downloads land in /data
# unless a command names another folder.
ENV HOME=/data
WORKDIR /data
VOLUME /data
ENTRYPOINT ["/usr/bin/megawren"]
LABEL org.opencontainers.image.source="https://github.com/def324/megawren" \
      org.opencontainers.image.description="Terminal file browser and transfer client for MEGA" \
      org.opencontainers.image.licenses="MIT"
