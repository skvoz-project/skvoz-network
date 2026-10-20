FROM moby/buildkit@sha256:a82d1ab899cda51aade6fe818d71e4b58c4079e047a0cf29dbb93b2b0465ea69 AS trust-seed
FROM ubuntu:24.04@sha256:a853f94d226358a79c740cfc7bce0c289748f3fe3488d921d038ccd752c61b60 AS fixture
ARG UBUNTU_SNAPSHOT=20261019T000000Z
# The official immutable image fixes the trust bytes. Copy no BuildKit programs.
COPY --from=trust-seed /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
# Signed immutable package indices keep this fixture recipe independent of
# current noble updates. Products and NATS are separate verified inputs.
RUN rm -f /etc/apt/sources.list /etc/apt/sources.list.d/ubuntu.sources \
    && printf '%s\n' \
      "deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}/ noble main universe" \
      "deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}/ noble-updates main universe" \
      "deb [signed-by=/usr/share/keyrings/ubuntu-archive-keyring.gpg] https://snapshot.ubuntu.com/ubuntu/${UBUNTU_SNAPSHOT}/ noble-security main universe" \
      > /etc/apt/sources.list \
    && apt-get -o Acquire::Check-Valid-Until=false update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      iproute2 nftables conntrack ca-certificates util-linux openssl \
      dbus polkitd systemd-resolved \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --gid 10001 skvoz \
    && useradd --uid 10001 --gid 10001 --no-create-home skvoz
COPY helper/polkit/org.skvoz.network.policy /usr/share/polkit-1/actions/org.skvoz.network.policy
RUN chmod 0644 /usr/share/polkit-1/actions/org.skvoz.network.policy \
    && printf '%s\n' 'polkit.addRule(function(action, subject) { if (action.id == "org.skvoz.network.manage" && subject.user == "skvoz") return polkit.Result.YES; });' \
      > /etc/polkit-1/rules.d/49-skvoz-fixture.rules \
    && chmod 0644 /etc/polkit-1/rules.d/49-skvoz-fixture.rules
