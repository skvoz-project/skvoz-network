# Короткая IP qualification

Этот fixture проверяет готовую цепочку runtime/helper/Core через настоящий
kernel TUN, TLS/NATS, NAT44, routed IPv6 и DNS. Это ограниченный функциональный
сценарий: по8192 TCP bytes и257 UDP bytes на семейство, DNS A/AAAA,
device-bound physical bypass/DNS, broker exception и runtime SIGKILL.
Speed, soak, capacity и полный исторический fault matrix сюда не входят.

Все проверки написаны на Rust: [actors](../../helper/tests/ip_topology.rs),
[TLS/config preparation](../../helper/tests/support/ip_prepare.rs),
[traffic/DNS endpoint](../../helper/tests/support/ip_traffic.rs).
Тот же probe содержит [host metadata proof](../../helper/tests/support/ip_host.rs):
это чтение Docker inspect и host `/proc`/cgroup2, без запуска functional actors.
Активация client helper использует общий owning fixture
[UID10001/polkit owner](../../helper/tests/kernel_client.rs).
Probe зависит от contracts/native и test-only pinned socket2; Core engine,
NATS library и runtime source не компилируются в этот probe.

## Входы

Подготовьте fixture image по [Dockerfile](Dockerfile) в ограниченном builder.
Base image и Ubuntu package snapshot закреплены; recipe не содержит compiler,
runtime/helper/Core или NATS binary. Зафиксируйте полученный image digest как
явный `SKVOZ_IP_IMAGE`. Существующий локальный image может использоваться только
как отдельный явно зафиксированный вход проверки. Compose ничего не скачивает.
Recipe и конкретный image требуют собственной проверки доступности/сборки.

Для первого HTTPS APT запроса recipe копирует только CA bundle из отдельного
официального `moby/buildkit` image с полным OCI digest
`a82d1ab899cda51aade6fe818d71e4b58c4079e047a0cf29dbb93b2b0465ea69`.
Digest закрепляет и bytes `/etc/ssl/certs/ca-certificates.crt`; BuildKit programs в
fixture не копируются. Это публичный воспроизводимый trust input: host CA,
private certificates и локальные unpublished images не используются.
Выбор distro trust candidate основан на [upstream Dockerfile](https://github.com/moby/buildkit/blob/v0.28.1/Dockerfile):
default Alpine image устанавливает git/openssl. Наличие CA bundle по выбранному
пути и успешный COPY должен подтвердить actual build; отдельная предварительная
acquisition не требуется, а отсутствующий файл останавливает recipe.
Затем APT обращается напрямую к HTTPS snapshot с проверкой TLS и подписи Ubuntu
archive keyring. Полный image digest служит pin trust seed; отдельный неизвестный
checksum файла не подставляется. Обычная установка `ca-certificates` формирует
финальный system trust. Доступность этих pinned inputs проверяет actual build.

В подготовленном build environment с лимитами создайте отдельную временную
копию исходников и установите `autobins = false` в её `helper/Cargo.toml`
(секция `[package]`). Это отключает автоматическую сборку product helper при
сборке integration tests; исходный checkout и готовые продукты не меняются.
В этой копии соберите только probe:

```sh
cargo test -p skvoz-network-helper --test ip_topology --release --locked --no-run
```

Передайте абсолютные пути:

- `SKVOZ_IP_PROBE`: полученный executable `ip_topology-*`.
- `SKVOZ_NATIVE_ROOT`: проверенный распакованный [общий Linux bundle](../../docs/artifacts.md)
  с сохранёнными `bin/`+`lib/` и directory mode0755. Product bytes не пересобираются.
- `SKVOZ_NATS_BINARY`: NATS2.15.1 executable с SHA256
  `baab2eecb5a8ebdabf6edd3e22756f89805e3184468653ba67185adf5a3b57eb`.
- `SKVOZ_BLOCK_DEVICE`: backing block device для действительного I/O cap.
- `SKVOZ_IP_EVIDENCE`: новый owning evidence directory.
- `SKVOZ_COMPOSE` (необязательно): абсолютный путь к явно выбранному Compose
  executable. Без него используется обычный `docker compose`. Выбранный путь
  используется во всех командах, включая cleanup, через тот же deadline/PID
  accounting; фиксируйте версию и SHA256 вместе с входами проверки.

Все actors сверяют runtime/helper/Core hashes с compiled `native-profile.json`.
Runtime запускается как UID10001/caps0, helper — root/NET_ADMIN only; app/helper
разделяют обычные DTO и проходят настоящую SO_PEERCRED/polkit авторизацию.
`LD_LIBRARY_PATH` отсутствует: loader использует сохранённый install layout.

## Ограниченный запуск

[Compose](compose.yaml) создаёт только task-owned internal dual-stack underlay и
egress bridges, без published ports, host network, SYS_ADMIN или Docker socket.
Фиксированные fixture subnets должны быть свободны; при конфликте запуск должен
остановиться. Существующие networks/routes/firewall не изменяются вручную.

Prepare service использует до4CPU/3GiB/swap0/128PIDs и8+2MiB/s I/O. Он завершает
private TLS/config volume до запуска любых actors. Одновременно работают:

| Actor | CPU | RAM | PIDs | Read/write I/O |
| --- | ---: | ---: | ---: | --- |
| Server | 1.5 | 1GiB | 96 | 3MiB/s +768KiB/s |
| Client | 1.5 | 1GiB | 96 | 3MiB/s +768KiB/s |
| Broker +echo/DNS | 1 | 512MiB | 64 | 2MiB/s +512KiB/s |

Суммарно4CPU,2.5GiB,swap0,256PIDs,8+2MiB/s. Все fixture daemons, probes,
product processes и healthchecks находятся под этими caps. Rust actors сверяют
реальные cgroup/capability values. Supervisor capabilities относятся только к
fixture setup; продуктовые дети отдельно подтверждают узкие полномочия.

Каждая из четырёх ролей сначала удерживается перед envelope и функциональным
кодом. Runner сохраняет inspect с точным container ID и `State.Pid`, затем тем же
`SKVOZ_IP_PROBE --host-proof` проверяет настоящий host cgroup2 mount и всю цепочку
предков PID. Изолированный cgroup namespace внутри actor не используется как
доказательство host limits. Docker ancestor обязан иметь точные CPU/RAM/swap/PID/
cpuset/I/O caps роли; process capabilities/NNP, swap и OOM counters также проверяются.
Actual Docker parents должны оставлять не менее8 logical CPU и6GiB RAM по
принятому правилу половины доступных ресурсов, независимо от ancestry caller.
Backing device major:minor определяется по явно заданному block device.

Перед release повторяются свежий inspect и `--host-recheck`: container ID,
host PID/starttime, boot и cgroup должны остаться прежними. Prepare проходит этот
gate отдельно, завершается успешно и удаляется до старта endpoint. Затем runner
последовательно открывает endpoint, server и client; endpoint/server должны
достичь health-ready до следующей роли. Gate не применяется к служебным
child/profile/shutdown режимам. Публичный runner не требует локальных adapters,
Ruby или sibling repositories. Отсутствующий host `/proc`, неполный cgroup mount
или недоступные ancestor files означают отказ до release.

После явного выделения очереди:

```sh
scripts/qualify-ip.sh
```

Shell содержит только lifecycle glue. Исходный предел340s состоит из240s работы,
80s cleanup и20s для финального kernel read/status; он не продлевается после
ошибки или timeout. Runner сохраняет logs, запрашивает реальный server shutdown
и удаляет только свой Compose project с containers/networks/private volume.
До создания проверяется отсутствие объектов с его точным project label.
`down` получает до45s, metadata/absence queries — до10s каждая, с отсечением
по оставшемуся исходному cleanup reserve. Временный приватный `DOCKER_CONFIG`
и локальный Unix `DOCKER_HOST` действуют и при запуске, и при финальных queries;
пользовательские context/TLS overrides не наследуются.
Успех требует свежих успешных daemon queries с пустыми списками всех project
containers/networks/volumes. Тот же probe через `--host-cleanup` дополнительно
требует отсутствия каждого записанного Docker cgroup, читаемого parent
`cgroup.procs` и завершения первоначального host PID (с учётом PID reuse).
Отсутствующая proof-запись созданного контейнера, timeout удаления или проверки означает
`cleanup=unknown`, ненулевой exit и сохранённые evidence/project name.

Kernel journal должен содержать доступную запись текущего boot. Runner сохраняет
её cursor, при каждом ограниченном чтении проверяет точное возвращение к этой
записи и ищет новые storage errors и hung-task сообщения вида
`INFO: task rustc:1234 blocked for more than 120 seconds`, включая priority3. Ошибка чтения/поиска, исчезнувший cursor,
смена boot, нетекстовая запись или превышение4096 records/1MiB snapshot
останавливают qualification. Во время длинных команд runner проверяет живость
watcher; его неожиданное завершение прерывает работу с NONPASS. Первый отказ
чтения или найденная ошибка защёлкивает NONPASS и один раз посылает TERM
контроллеру. Watcher продолжает собирать журнал до cooperative stop, включая
cleanup; это не повтор функциональной работы. Monitor сохраняет
исходный hard deadline и работает во время cleanup. Обычная остановка идёт через
stop marker: текущий journal read завершается с reap команды, watcher подтверждает
остановку и выходит. Затем выполняется независимое финальное чтение и сверяются
PID-журналы started/reaped. TERM/KILL используется только аварийно с NONPASS.
Начальная запись служит границей и сама не относится к новому запуску. `status`
остаётся `unknown`, если hard deadline не позволил завершить проверку. Нужны Docker Compose, доступный kernel journal, TUN и перечисленные
namespace-local sysctl из Compose; отсутствующие prerequisites останавливают
qualification и не разрешают менять host settings.

Server запускается через короткий [fixture supervisor](server-supervisor.sh).
Он атомарно публикует точный exit bounded actor invocation в `/run/topology/server-actor.exit`
через временный файл и rename в том же каталоге;
ненулевой status остаётся ошибкой. После чистого выхода actor supervisor держит
container до cleanup или исходного work cutoff. Это даёт Docker exec requester
время завершиться после ответа `clean`, сохраняя настоящие Rust assertions
runtime/helper shutdown и owned kernel cleanup. Supervisor не создаёт `ready` и
не объявляет успех вместо actor; capabilities и caps роли остаются прежними.

По умолчанию `SKVOZ_IP_SCOPE=full`. Для отдельной проверки Server shutdown без
повторения packet-сценария задайте `SKVOZ_IP_SCOPE=server_shutdown`: runner выполняет
prepare, endpoint и server, затем тот же существующий Rust `--shutdown` handshake.
Client в этом scope не создаётся. Его успешный результат относится только к Server
closure. Prepare budget прежний; одновременно живущие endpoint+server ограничены
2.5CPU/1.5GiB/swap0/160PIDs и5MiB/s read+1.25MiB/s write. Полный scope сохраняет
прежний общий actor budget. Оба scope требуют отдельного разрешённого запуска.

Runner сохраняет stdout/stderr/CLI exit client Docker wait (в `full`), Server exec
и чтения actor status, независимо от их успеха. Успех closure требует одновременно
requester exit0 (получен и проверен Rust `clean`) и actor exit0 (пройдены реальные
product shutdown/helper reaps/owned cleanup assertions). Unknown или ненулевой
status не заменяется успехом. Отдельные partial observations не превращают
предыдущий неуспешный gate в PASS.

Перед запуском helper/runtime Server fixture восстанавливает namespace-only IPv6
default через `2001:db8:900::1` на фактическом egress interface. Это prerequisite
helper preflight для routed IPv6; host routes не меняются. Ограниченные команды
сохраняют только address/IPv4+IPv6 routes и forwarding/fragment sysctl values,
затем проверяются egress IPv6 address, default device/gateway и допустимые
namespace limits. Profile, credentials и TLS material в snapshot не попадают.
Проверки helper и его исходный command budget остаются прежними.

## Наблюдаемый результат

Client получает dual-stack CONFIGURED, передаёт helper-owned TUN через ATTACH_IP,
достигает ACTIVE и проверяет TCP/UDP payload. Endpoint сообщает source address:
для IPv4 он должен совпасть с Server egress NAT44, для IPv6 — с выданным grant.
Resolved A/AAAA запросы доходят до настоящего DNS responder; тот проверяет
транзитный NAT44 source. Прямой TCP/DNS через underlay interface запрещён,
broker TCP exception остаётся доступным.

После SIGKILL настоящего runtime guard и запрет physical bypass проверяются до
ABORT. Затем ABORT/owner EOF, новый авторизованный helper owner, RECOVER и явный
RESTORE сохраняют guard до намеренного восстановления. После RESTORE отсутствуют
owned TUN/nft/proto186 routes и снова доступен physical baseline. Отдельный
server shutdown подтверждает cleanup его owned ресурсов. Только фактически
успешный запуск квалифицирует эти source/profile/bytes; наличие fixture кода
само по себе не является результатом.
