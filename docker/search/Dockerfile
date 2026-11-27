# syntax=docker/dockerfile:1
#
# The search server of fvoci: OpenSearch, one node, with the Korean analysis plugin nori
# and the security plugin (HTTPS and a user of its own), and no other plugin.
#
#   docker build -t fvoci-search docker/search
#
# docker/compose.yaml runs it as the service `search`, on its network only. At start
# (search-entrypoint.sh) it makes, once, the certificates in the volume at config/tls
# (the node's, for the name `search`, and an administrator's) and writes their
# authority's certificate to ca-out/ for fvoci to trust; at every start it sets the
# user `fvoci`, its role and the password from the file FVOCI_SEARCH_PASSWORD_FILE
# names. The certificates stay as long as that volume.
#
# The same file builds linux/arm64 and linux/amd64: the base is the list of both,
# named by its digest.

# The version of the digest above, for the label.
ARG OPENSEARCH_VERSION=3.9.0
FROM opensearchproject/opensearch:3.9.0@sha256:3b55f6b6249cfdb92b52ee59adc8a266203ba9ef9f5d82544fad6f1af2f3609e

USER root
# openssl makes the certificates at the first start.
RUN dnf install --assumeyes openssl && dnf clean all
USER opensearch

# Only the security plugin stays: the others take memory and start-up time and fvoci
# uses none of them. nori comes in the version of the server.
RUN cd /usr/share/opensearch \
    && for plugin in plugins/*; do \
        name=$(basename "$plugin"); \
        if [ "$name" != opensearch-security ]; then rm -rf "$plugin" "config/$name"; fi; \
    done \
    && bin/opensearch-plugin install --batch analysis-nori \
    && mkdir -p config/tls ca-out \
    && chmod 700 config config/tls

COPY --chown=opensearch:opensearch --chmod=600 opensearch.yml /usr/share/opensearch/config/opensearch.yml
COPY --chown=opensearch:opensearch --chmod=755 search-entrypoint.sh /usr/share/opensearch/search-entrypoint.sh

# The demo configuration of the security plugin has well-known passwords and keys:
# never installed here.
ENV DISABLE_INSTALL_DEMO_CONFIG=true \
    DISABLE_PERFORMANCE_ANALYZER_AGENT_CLI=true

HEALTHCHECK --interval=10s --timeout=5s --start-period=120s --retries=5 \
    CMD ["curl", "--silent", "--fail", "--config", "/usr/share/opensearch/config/healthcheck.curlrc", "https://localhost:9200/_cluster/health?wait_for_status=yellow&timeout=2s"]
ENTRYPOINT ["/usr/share/opensearch/search-entrypoint.sh"]

ARG OPENSEARCH_VERSION
ARG FVOCI_BUILD_HASH=
ARG FVOCI_CREATED=
ARG FVOCI_VERSION=
LABEL org.opencontainers.image.title="fvoci-search" \
      org.opencontainers.image.description="OpenSearch ${OPENSEARCH_VERSION} with nori, for fvoci" \
      org.opencontainers.image.source="https://github.com/AISFlow/fvoci" \
      org.opencontainers.image.licenses="MIT" \
      org.opencontainers.image.revision="${FVOCI_BUILD_HASH}" \
      org.opencontainers.image.created="${FVOCI_CREATED}" \
      org.opencontainers.image.version="${FVOCI_VERSION}"
