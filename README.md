# Общий сетевой модуль

`skvoz-network` 0.7.0 содержит переносимые сетевые состояния и общий Linux/Android runtime,
который использует один [Core/NatsRuntime](https://github.com/skvoz-project/skvoz-core/blob/main/docs/nats-runtime.md). Core
передаёт непрозрачные байты; сетевой модуль проверяет network v6, управляет
TCP-потоками и полными IPv4/IPv6-пакетами. Локальный контракт, строгая
конфигурация и команды описаны в [руководстве runtime](docs/network-runtime.md).

`RuntimeHandle` и `skvoz-network-runtime` используют один actor. Локальный
HELLO не ждёт подключения NATS. Клиент выбирает взаимоисключающие режимы
`proxy` и `ip`; сервер принимает оба сетевых профиля через тот же Core.
Клиент открывает TCP/VPN-сеансы непосредственно к peer 0 полного сервера.
Сервер принимает только текущий локальный набор назначенных устройств;
UPDATE_DEVICES отзывает прежние Core-поколения и подтверждает завершение
принадлежащих им TCP/IP/helper ресурсов. Обычные NATS entry соединяются
нативными leaf links, как описано в [серверном руководстве](https://github.com/skvoz-project/skvoz-server/blob/main/docs/server-connector.md).
HTTP/CONNECT/SOCKS5 CONNECT разбираются внутри Rust; payload не передаётся
через управляющий JSON. OPEN_TCP возвращает owned Unix FD после удалённого
ACCEPT. Остаток частичного SEND сохраняется, кредит принимаемых TCP-байтов
возвращается только после завершённой записи в локальный сокет.

IP-сессия согласует CONFIG, открывает все каналы и проходит READY/ACTIVE.
Пакеты проверяются по семейству, длине, MTU и source grants. Сессия привязана
к PeerId, epoch/incarnation Core и своему идентификатору. Серверный runtime
единолично владеет helper channel: RESERVE предшествует CONFIG, ACTIVATE
подтверждается до ACTIVE, RETIRE закрывает доступ прежней сессии. Linux
маршруты, firewall и постоянные назначения принадлежат отдельному helper.

`enqueue_packet` сохраняет остаток частичной отправки Core. `poll_packet`
передаёт пакет I/O с отметкой окончания записи, а `complete_packet` возвращает
кредит после полной записи либо осознанного отбрасывания. Извлечение или
копирование пакета не подтверждает потребление; позднее завершение не обходит
более ранний незавершённый пакет. Очереди и общий ledger ограничивают байты и
число записей. Kernel socket buffers и память системного resolver учитываются
отдельно при квалификации.

Граница Linux/Android FD/TUN и Linux process/create/SCM_RIGHTS находится в [native/](native/README.md),
привилегированные операции — в [helper/](helper/README.md), тонкая C ABI1 —
в [ffi/](ffi/README.md). Основная библиотека сохраняет запрет `unsafe`.
FFI не создаёт второй engine или собственную очередь событий. Недостаточный
буфер сохраняет ту же authoritative запись и FD до успешного получения.

`portable-runtime` включает тот же actor для Linux и Android; `linux-runtime`
сохраняет Linux CLI host. Android допускает только client role без helper,
получает проверенный owned TUN FD от [мобильного клиента](https://github.com/skvoz-project/skvoz-client-android/blob/main/docs/android-client.md).
Общий enrollment4 используется Ubuntu и Android; host lifecycle/trusted anchor
storage остаются ответственностью платформы. Core wire2, API1 и ABI1 сохранены; network6 и StartupConfig3 проверяются явно.
При `AuthorizationViolation` подключение enrollment повторяется ещё два раза
с паузами 1 и 2 секунды: NATS может начать принимать соединения до запуска
Auth Callout. Сохраняются проверка TLS и общий таймаут enrollment 20 секунд;
после исчерпания повторов отказ авторизации возвращает `authentication_failed`.

Основная поставка идёт через GitHub Actions: push в `main` получает точный
закреплённый Core release, собирает Linux supplier и публикует native archive,
Desktop/portable SDK и `SHA256SUMS` под `network-v0.7.0-<полный40SHA>`.
Server, Client Service и Desktop получают готовый pinned native archive в своих
workflows; Android native supplier использует portable SDK и отдельные Core
targets. PR проверяет сборку без публикации. Первый выпуск требует опубликованного
Core и явного обновления pins; порядок и команда описаны в
[руководстве выпусков](docs/artifacts.md#github-releases-и-обновление-core).
Локальные команды ниже нужны для разработки и дополнительной проверки supplier.

Для явной supplier сборки сначала подготовьте закреплённую Core library по
[описанию артефактов](docs/artifacts.md#явная-сборка-поставщика). Source SDK уже
включён в `vendor/skvoz-core`; engine Core здесь отсутствует. Каталог готового
артефакта обязан содержать совпадающие с target pin `artifact.json` и `lib/`.
Отсутствие или несовпадение артефакта завершает сборку ошибкой.

Из корня репозитория:

```sh
export SKVOZ_CORE_ARTIFACT=/path/to/verified-core-artifact
cargo build -p skvoz-network --features linux-runtime --locked
cargo test -p skvoz-network --features linux-runtime --locked
cargo test -p skvoz-network-native --locked
```

Исполняемый файл получает private JSON и подключённый управляющий Unix FD:

```sh
skvoz-network-runtime --config private.json --control-fd 3
```

Для сервера с IP backend дополнительно обязателен `--helper-fd 4`.
Launcher передаёт назначенные FD исключительно дочернему процессу; обычные
числа чужих FD вместо SCM_RIGHTS не принимаются. `--help` и `--version`
работают без подключения NATS. STOP подтверждается после ограниченного
завершения собственных потоков; ошибка или deadline не выдаётся за успешную
очистку. Потеря владельца останавливает runtime, helper закрывает live доступ.

Детерминированные checks и отдельные реальные TLS/NATS/TUN cases описаны в
[руководстве проверки](docs/verification.md). Они имеют разные пределы доказательства.

Репозиторий поставляет [строгие contracts](contracts/README.md),
[host enrollment SDK](client/README.md), [готовые Linux outputs](docs/artifacts.md)
и [исходные SDK](docs/artifacts.md#исходные-sdk). Обычные Server/Desktop/Client Service packaging
используют pinned archive; supplier Cargo build является отдельным действием.
