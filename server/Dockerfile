FROM golang:1.26.10-alpine AS build
WORKDIR /src
COPY server/go.mod server/go.sum ./server/
RUN cd server && go mod download
COPY server/ ./server/
COPY LICENSE THIRD_PARTY_NOTICES.md THIRD_PARTY_LICENSES.md ./
# Keep the exact corresponding source and dependency notices with the image.
RUN cd server && go mod vendor && \
    CGO_ENABLED=0 go build -mod=vendor -trimpath -ldflags="-s -w" -o /voice-server ./cmd/voice-server
RUN tar -czf /corresponding-source.tar.gz LICENSE THIRD_PARTY_NOTICES.md THIRD_PARTY_LICENSES.md server

FROM scratch
COPY --from=build /voice-server /voice-server
COPY --from=build /src/LICENSE /src/THIRD_PARTY_NOTICES.md /src/THIRD_PARTY_LICENSES.md /usr/share/licenses/edenvoice/
COPY --from=build /corresponding-source.tar.gz /usr/share/edenvoice/corresponding-source.tar.gz
COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
LABEL org.opencontainers.image.source="https://github.com/SmileSion/EdenVoice" \
      org.opencontainers.image.licenses="GPL-3.0-only"
EXPOSE 9000/tcp 9001/udp
ENTRYPOINT ["/voice-server"]
