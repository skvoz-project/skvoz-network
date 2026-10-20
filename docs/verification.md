# Проверка Network

Source supplier использует готовый Core SDK/library. Обычные checks не требуют
привилегий и не меняют host networking:

Сначала подготовьте [pinned Core SDK и готовую библиотеку](artifacts.md) и задайте
`SKVOZ_CORE_ARTIFACT` на проверенный каталог с `artifact.json` и `lib/`.

```sh
cargo test --locked -p skvoz-network-contracts -p skvoz-network-native -p skvoz-network-helper
cargo test --locked -p skvoz-network --features linux-runtime
cargo test --locked -p skvoz-network-client -p skvoz-network-ffi
cargo clippy --locked --workspace --all-targets --features linux-runtime -- -D warnings
```

Rust qualification содержит `network_probe` для TUN/NATS, authenticated adversary
и Core echo diagnostic, а также `network_ip_credit` для bounded queues/credit.
Реальные проверки запускаются явно с provisioned TLS/NATS fixture в одноразовом
окружении, с resource caps и конечным сроком:

```sh
cargo test --locked -p skvoz-network-qualification --features real-nats --test network_ip_credit
cargo test --locked -p skvoz-network --features linux-runtime --test runtime_access -- --ignored
cargo test --locked -p skvoz-network-ffi --test real_nats -- --ignored
```

`runtime_access` получает `SKVOZ_RUNTIME_FIXTURE`, `network_ip_credit` —
`SKVOZ_NATS_URL`, `SKVOZ_NATS_CA`, `SKVOZ_NATS_RUN_TOKEN` и
`SKVOZ_NATS_P0_PASSWORD`/`P1_PASSWORD`. FFI real transport получает
`SKVOZ_NETWORK_SERVER_CONFIG`, `SKVOZ_NETWORK_CLIENT_CONFIG`,
`SKVOZ_NETWORK_ECHO_HOST` и необязательный `SKVOZ_NETWORK_ECHO_PORT` (4444).
Private config/credentials находятся в изолированном каталоге0700 с файлами0600.

`qualification/src/bin/qualify.rs` создаёт приватный TLS broker и временные
owning profiles, запускает переданные готовые Rust test executables, затем
завершает broker и удаляет fixture. Требуются OpenSSL и точный NATS2.15.1.
`runtime_smoke` проверяет поставленный `SKVOZ_RUNTIME_EXECUTABLE` через API1,
реальный TLS, owned TCP FD, двоичный echo, half-close и owner EOF. Для проверки
install closure запускайте из сохранённой структуры `bin/`+`lib/` с удалённым
`LD_LIBRARY_PATH`.

Kernel checks исполняются отдельно в одноразовой private network namespace.
`helper/tests/kernel_lifecycle.rs` проверяет реальный helper, dual-family grants,
kernel routes, owner EOF, SIGKILL/recovery и FD lifecycle. Отдельный Rust runner
`kernel_client` входит в fixture только при `SKVOZ_KERNEL_FIXTURE=1`; нужны
UID10001, fixture polkit rule, DBus, systemd-resolved, NET_ADMIN и TUN. Listener
создаётся реальным app UID и передаётся helper как systemd-equivalent activation;
HELLO проходит обычную SO_PEERCRED/polkit авторизацию. Проверяются guard,
broker route exception, DNS binding, foreign handle, ABORT/EOF и crash recovery.
Эти исходники требуют отдельной kernel qualification; обычный unit run их не
подтверждает. Namespace-local sysctl, caps, deadline и лимиты каждого процесса
принадлежат внешнему подготовленному fixture envelope.

Короткий [IP topology fixture](../qualification/ip/README.md) проверяет
сквозные NAT44/routedIPv6, TCP/UDP и DNS A/AAAA assertions на готовые shipping
runtime/helper/Core bytes. Он проверяет physical bypass/DNS denial, broker
exception, runtime SIGKILL, guard до ABORT и RECOVER/RESTORE с cleanup.
Три одновременных actors ограничены суммарно4CPU/2.5GiB/swap0 и8+2MiB/s;
отдельный prepare service завершается до их запуска. Image и готовые native
inputs задаются явно; существование этого source fixture не означает, что его
kernel/profile checks уже исполнились.

Для Linux выполнены реальные helper lifecycle и client guard/DNS/recovery
проверки на закреплённых runtime/helper/Core bytes. Короткий IP fixture отдельно
подтвердил TCP/UDP IPv4/IPv6 payload, NAT44/routedIPv6 source, DNS A/AAAA,
physical bypass denial и guard после runtime crash. Завершение server runtime/
helper и очистка ресурсов подтверждены отдельной focused проверкой.
Общий topology controller run не получил итоговый PASS; эти результаты не
подтверждают полный fault matrix или производительность. Изменение исходников,
профиля или native bytes требует соответствующей новой проверки.

Каждый запуск подтверждает только выбранные assertions и точные native bytes.
Unit tests не подтверждают NAT44/routedIPv6/resolved/polkit/crash qualification;
для этих гарантий нужны соответствующие реальные fixtures выше.

Проверка схем, ABI или compilation сама по себе не устанавливает platform,
installation, kernel isolation или throughput guarantees. Проверка готового
общего Linux archive на Debian/Ubuntu и взаимодействия продуктов выполняется
по точным digest отдельно; результат относится только к проверенному profile.
