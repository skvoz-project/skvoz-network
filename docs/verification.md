# Проверка Network

Cargo запускается из `src/` с Rust1.92.0 и общим lock. Runtime/FFI получают
[закреплённую готовую Core library](artifacts.md) через `SKVOZ_CORE_ARTIFACT`;
Core engine здесь не собирается. Полная локальная цепочка:

```sh
docker/local_build.sh /path/to/new-network-build
cd src
SKVOZ_TEST_READY_NATIVE_ARCHIVE=/path/to/new-network-build/skvoz-network-0.7.0-x86_64-unknown-linux-gnu.tar.gz \
  cargo test -p skvoz-network-helper --features fixture --test kernel_fixture \
  --release --locked -- --ignored --test-threads=1 --nocapture
```

Скрипт только вызывает Buildx target `artifacts`: три архива и `SHA256SUMS` после
обычных Cargo checks. Для независимого SDK export достаточно
`TARGET=sdk-artifacts docker/local_build.sh /path/to/sdk-output`; host Rust/Core
binary/NATS/kernel не нужны. `TARGET=linux-artifacts` выдаёт только Linux archive.
CI выбирает эти независимые targets, затем Linux job отдельно вызывает тот же
owning Rust test перед upload. Publication ждёт оба jobs.

Для полного запуска нужны Ubuntu24.04 Linux host, Rust1.92.0, C compiler, curl,
tar, `/bin/kill`, Docker Buildx и `/dev/net/tun`. Dockerfile владеет pinned
preparation, компиляцией и обычными Cargo tests/fmt/Clippy. `linux-artifacts`
получает архив через успешный `ordinary-checks`; Bookworm product ABI и Ubuntu24
проверок сохраняются. Cargo использует свой стандартный target/cache;
`CARGO_TARGET_DIR` можно явно направить в выбранный внешний cache.

Kernel fixture требует только готовый archive. Внутри обычного Rust test она
вызывает стандартный Cargo для двух owning child tests, потребляет JSON напрямую
и ждёт compiler exit. Testcontainers строит OS-only environment из основного
Dockerfile; ни shell launcher, ни workflow не управляют image tags/metadata.
Private scratch, compiler process group, image/containers/networks/volume принадлежат
fixture и явно удаляются после сценария или ошибки.

Основные языковые проверки выполняются в target `ordinary-checks`. Их можно
запустить отдельно в подготовленной Ubuntu-среде с готовым Core:

```sh
cd src
features=skvoz-network/linux-runtime,skvoz-network/real-nats,skvoz-network-helper/fixture
cargo fmt --all -- --check
cargo test --workspace --features "$features" --release --locked -- --test-threads=1
cargo clippy --workspace --features "$features",skvoz-network-qualification/experiments \
  --all-targets --release --locked -- -D warnings
```

| Проверки | Владелец и обычный CI |
| --- | --- |
| Network6, Startup3/API1/helper/enrollment DTO, packet validation и policy | `contracts/tests/` и private units; Cargo workspace tests |
| Engine budgets/order/credit, runtime owner/cancellation/deadlines, TCP/Proxy | `network/src/` private units и `network/tests/process_lifecycle.rs`; Cargo workspace tests |
| Unix/SCM_RIGHTS, framing/deadlines, anchored config/state и явно вызванный Rust FD child | `native/tests/` и private units; Cargo workspace tests |
| Helper reserve/activate/retire, journal/registry, firewall grammar | `helper/tests/` и private units; Cargo workspace tests |
| Host SDK subprocess identity, bounded output, typed errors, cancellation/reap | `client/tests/process.rs`; Cargo workspace tests |
| ABI generations/handles, retained message/FD, strict arguments, C header compilation | `ffi/tests/` и private units; Cargo workspace tests |
| Pins/acquisition failures, exact SDK closure/ownership и locked exported workspace | `tools/tests/`; SDK job и workspace tests |
| Real TLS/NATS readiness/wake/cancellation, family negotiation, remote profiles, IP credit/backpressure, raw-peer session/source isolation, current device revocation | `network/tests/{runtime_readiness,network_ip_credit,ip_isolation,runtime_access}.rs`; owning tests с `TlsFixture` |
| Real TLS/NATS ABI binary echo/half-close | `ffi/tests/real_nats.rs`; owning test с `TlsFixture` |
| Ready-byte native archive closure | `tools/tests/native_bundle.rs`; явный Cargo `--ignored` с готовым Linux archive/hash |
| Installed runtime API1, owned TCP FD, binary echo/half-close и owner EOF | `network/tests/installed_runtime.rs`; явный Cargo `--ignored`, child без `LD_LIBRARY_PATH` |
| Real TUN, server/client helper grants/routes/owner EOF/SIGKILL/guard/DNS/polkit/foreign UID/restore | `helper/tests/kernel_fixture.rs` запускает owning executables `native/tests/linux_tun.rs`, `helper/tests/kernel_lifecycle.rs` и client actor |
| Full NAT44/routed IPv6, TCP/UDP, DNS A/AAAA, physical bypass denial, broker exception, runtime crash/RECOVER/RESTORE и server closure | тот же Testcontainers test, actors в `helper/tests/actors/` |

`qualification::TlsFixture` — общая Rust test dependency. Каждый owning test
создаёт приватные TLS identities, NATS и echo listener, затем reap broker,
останавливает/join echo thread и удаляет private directory. OpenSSL используется
для сертификатов. Точный NATS2.15.1 скачивается один раз в Cargo target cache;
проверяются SHA256 архива и каждого повторно используемого executable. Directory
mode0700, credential files0600. Конфигурации передаются явно; глобальное
окружение процесса для настройки fixture не меняется.

`--no-run` внутри fixture только компилирует два child executables. Parent затем
действительно исполняет TUN/server helper checks с `--ignored`, проверяет обе
непустые result summaries, client completion marker и full IP marker. Actor bins
включаются только feature `fixture` и не поставляются. Отсутствующий готовый
archive — ошибка; скрытой product/Core сборки в fixture нет.

Target `test-environment` основного [Dockerfile](../docker/rust/Dockerfile)
как последний OS-only target содержит закреплённый Ubuntu24.04 image и signed snapshot packages
ip/nft/conntrack/DBus/polkit/resolved. Candidate runtime/helper/Core, test binaries,
policy и проверенный NATS монтируются read-only. Rust test создаёт private input
volume и две internal dual-stack сети, назначает прежние статические адреса,
проверяет readiness и вызывает server shutdown. Реальный server actor должен
завершиться с exit0 после собственных cleanup assertions. Testcontainers handles
и дополнительная проверка exact ownership label обеспечивают явное удаление
всех своих контейнеров/сетей/volume и image, в том числе при неполном setup.
Compiler child сохраняется до wait/reap; deadline/cancellation останавливает
его собственную process group, затем ждёт завершения.

Actors не получают Docker socket, host network, опубликованные ports или
privileged mode. Сохраняются read-only rootfs, NNP, точные capabilities/UID,
TUN и namespaced fragmentation/forwarding sysctls. Docker API orchestration
выполняется только родительским host test. Testcontainers high-level lifecycle
дополнен узкими Bollard IPAM/attachment операциями для этой двухсетевой topology.

`network_probe` остаётся диагностическим TUN/Core echo экспериментом с feature
`experiments`; Clippy проверяет компиляцию, CI его не запускает. Performance,
Android/JNI/device и deployment не подтверждаются этой цепочкой. Публикация
ждёт SDK и все Linux Cargo gates, использующие те же готовые candidate bytes.
