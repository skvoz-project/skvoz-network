# Готовые Linux артефакты

Основной путь: Core Actions публикует проверяемый release; явное обновление
закрепляет выбранный Core release в этом репозитории; push Network в `main`
запускает supplier Actions и публикует Network release; workflows продуктов
получают конкретные закреплённые архивы. Локальная supplier сборка — отдельный
способ разработки и дополнительной проверки.

Общий профиль `x86_64-unknown-linux-gnu / linux-gnu-bookworm` предназначен для
одних runtime/helper/Core bytes в Server, Client Service и Desktop. Совместимость
конкретных outputs подтверждается отдельно; наличие исходников не является
квалификацией Debian/Ubuntu или kernel integration.

Архив содержит `bin/skvoz-network-runtime`, `bin/skvoz-network-helper`,
`lib/libskvoz_core.so`, `resources/systemd/`, `resources/polkit/`, `licenses/`,
`source-inputs.sha256` и `manifest.json`. Установка сохраняет всю структуру
под `/opt/skvoz-native`; публичные команды могут быть symlink на `bin/`.
Runtime использует RPATH `$ORIGIN/../lib`. Helper не связывается с Core/NATS.

Manifest schema1 фиксирует `component`, `version`, `target`, `profile`, `tuple`,
`source`, `files` и `dependencies`. Tuple содержит `api=1`, `network=6`,
`core=5.0.0`, `helper=0.6.0`, `core_abi=1`, `network_abi=1`. Каждый файл имеет
относительный `path`, `sha256` и числовой Unix `mode` (493 или420).
Core dependency содержит `component`, `version`, `abi`, `sha256` готовой `.so`.

`source.tree_sha256` — SHA256 файла `source-inputs.sha256`: отсортированные
строки `SHA256<TAB>относительный-путь<LF>` для manifests, lock, Rust source,
source SDK, installation resources, supplier scripts, `.cargo/`, owning workflows,
rust-toolchain и `native-profile.json` с точными toolchain/flags/profile inputs.
Поставщик выполняет inventory в owning Git checkout: учитываются cached и
untracked public inputs, существующие Git ignores исключают private material. `source.repository`
указывает Network, `source.revision` может быть null только у локального
незакоммиченного кандидата; выпуск фиксирует owning commit. Поставщик формирует `source`
с полями `repository`, `revision` и `tree_sha256`.

Consumer lock schema1 содержит ту же identity и `artifact:{url,sha256}`.
Относительный `file:` locator разрешается относительно каталога lock, независимо
от текущего рабочего каталога. Например, root `product-native-lock.json` использует
`file:artifacts/native/network-native.tar.gz`; lock `artifacts/network.lock.json`
для того же архива использует `file:native/network-native.tar.gz`.
Consumer проверяет закреплённый SHA256 всего архива до распаковки, затем
извлекает необходимые продукту файлы стандартным `tar`. Проверка хеша фиксирует
все bytes выбранного выпуска. Состав, manifest, target/ABI и runtime closure
проверяются поставщиком при сборке выпуска; продукт сохраняет собственные
проверки связывания, загрузки и версий. Ошибки останавливают packaging без source fallback.

## Получение готового native bundle

Обычная сборка получает архив по точному URL из собственного lock, сверяет его
закреплённый SHA256 и извлекает необходимые файлы стандартными средствами:

```sh
curl --fail --location --proto '=https' --proto-redir '=https' --max-time 120 \
  "$NETWORK_URL" --output network.tar.gz
printf '%s  %s\n' "$NETWORK_SHA256" network.tar.gz | sha256sum --check -
mkdir native
tar -xzf network.tar.gz -C native --no-same-owner --no-same-permissions \
  -- bin/skvoz-network-runtime bin/skvoz-network-helper lib/libskvoz_core.so
```

Значения URL/SHA256 берутся из просмотренного lock, а не из автоматически
выбранного `latest`. Для упаковки helper продукт также извлекает нужные
`resources/` и `licenses/`, сохраняет исходный manifest и нормализует права
executables0755/данных0644. При ошибке временное дерево удаляется, существующий
результат не заменяется. Получение не активирует службы и не собирает исходники.

Внутренний `skvoz-network-artifact` используется только самим поставщиком для
формирования manifest и подготовки bundle. Он не входит в релизный архив,
не загружается и не запускается продуктами. Собственный parser tar и отдельный
исполняемый установщик потребителю не нужны. Опубликованные native archives
сохраняют свои реальные hashes; обновление выбранного выпуска выполняется явно.

## GitHub Releases и обновление Core

Workflow `Network` на `main` и при ручном запуске получает закреплённый Linux
архив Core и собирает runtime/helper в Rust1.92.0 / Debian Bookworm. Pull request
выполняет ту же проверку и сборку с `contents: read`; публикация доступна только
отдельному job доверенного `main`. Сборка использует ресурсы GitHub runner;
каждый контейнер имеет deadline и проверку удаления. Локальные CPU/RAM/swap/PID/I/O
ограничения задаёт отдельное окружение проверки, они не являются условием CI.
Профиль требует GLIBC не новее2.34 для Core, runtime и helper.

Выпуск имеет неизменяемый тег `network-v0.7.0-<полный40SHA>` owning commit и
четыре файла:

- `skvoz-network-0.7.0-x86_64-unknown-linux-gnu.tar.gz`;
- `skvoz-network-sdk-0.7.0-desktop-host.tar.gz`;
- `skvoz-network-sdk-0.7.0-portable-supplier.tar.gz`;
- `SHA256SUMS` для трёх архивов.

Существующий тег останавливает публикацию; overwrite, перенос тега и moving
`latest` не используются. Native manifest и оба `sdk.json` фиксируют реальный
owning commit; грязный checkout при `GITHUB_SHA` отклоняется. Локальный кандидат
без чистого owning commit сохраняет `revision:null`. Новые outputs имеют свои
manifest hashes; исторические hashes и сведения о квалификации в
`native-profile.json` описывают прежние проверенные bytes и не квалифицируют
новую CI сборку автоматически.

Первый запуск требует реально опубликованного выпуска Core. Текущие локально
проверенные pins остаются действительными для своих bytes; до первого явного
обновления они не содержат выдуманного release URL/SHA архива и CI останавливается
с указанием bootstrap prerequisite. После выпуска Core выполните из корня
Network checkout; перед обновлением сохраните текущие изменения для review:

```sh
cargo build --release --locked -p skvoz-network-artifact
# По умолчанию используется настроенный gh; private repository требует доступа.
timeout --kill-after=5s 600s target/release/core-release refresh \
  "core-v5.0.0-<полный40SHA>"
git diff -- dependencies/core vendor/skvoz-core
```

Для публичного Core без авторизации добавьте `--public` перед `refresh` или
`fetch`: `core-release --public refresh "core-v5.0.0-<полный40SHA>"`. Этот режим
использует анонимные HTTPS-запросы `curl` к GitHub API и release assets. Ошибки
авторизованного `gh` не включают его автоматически; настройки и credentials
не меняются. Первый refresh допускается до первого commit Network: revision
будущего Network release определяется отдельно при сборке supplier из чистого
owning commit.

Команда принимает только точный тег. Она проверяет его lightweight ref против
owning commit через GitHub API, получает `SHA256SUMS` и все три Core archives,
проверяет digest архива, descriptor, source revision/tree, ABI1, target, shared
ELF, SDK inventory и Android profile. SDK всех targets обязан совпадать. Только
после всех проверок staged directories заменяют все Linux/Android pins,
Android descriptors и vendored SDK; ошибка замены вызывает rollback. Запускайте
одного writer: два каталога заменяются согласованно в одной операции команды,
но файловая система не предоставляет общего crash-atomic rename для них.
Если процесс прерван во время замены, сохраните и проверьте каталоги
`*.release-backup` и `.core-release-next` перед ручным восстановлением; повторный
запуск отклоняет такие остатки.

После review/commit обновлённых pins supplier получает только закреплённый target:

```sh
timeout --kill-after=5s 300s target/release/core-release fetch \
  x86_64-unknown-linux-gnu /path/to/new-verified-core
export SKVOZ_CORE_ARTIFACT=/path/to/new-verified-core
```

В ordinary CI используется прямой путь: `curl` по URL из pin, `sha256sum`
и `tar` для `artifact.json`/`lib/libskvoz_core.so`. `build.rs` и binding сохраняют
проверки interface/manifest и bytes готовой библиотеки при связывании.

Отдельный `fetch` для локальной диагностики дополнительно сверяет descriptor с pin, а SDK — с
локальным `vendor/skvoz-core`. Он не изменяет pin и не перезаписывает выходной
каталог. Отсутствующий asset, несовпадение SHA/identity/profile или недоступный
upstream завершают команду ошибкой; сборки Core из исходников здесь нет.
Для следующего выпуска повторите `refresh` с явно выбранным новым тегом и
проверьте diff перед публикацией Network. Команды не создают commits и releases.

## Явная сборка поставщика

Source SDK Core готовится отдельно из pinned supplier bundle в
`vendor/skvoz-core`; готовая библиотека имеет собственный проверенный digest.
`dependencies/core/<target>.json` закрепляет artifact ID, library/source/SDK hashes;
`build.rs` сверяет выбранный manifest, а Cargo binding проверяет реальные library
bytes и связывает их по явному `SKVOZ_CORE_ARTIFACT`. Target без готового pin
завершается ошибкой. Обычный продукт не
запускает команды ниже и не содержит engine Core.

```sh
export SKVOZ_CORE_ARTIFACT=/path/to/verified-core-artifact
cargo build --release --locked -p skvoz-network --features linux-runtime
cargo build --release --locked -p skvoz-network-helper -p skvoz-network-artifact
scripts/bundle-native.sh target/release/skvoz-network-artifact \
  target/release /path/to/verified/libskvoz_core.so /path/to/network-native.tar.gz
```

Сборка выполняется только в согласованном подготовленном окружении с реальными
resource caps и deadline. `bundle-native.sh` ничего не компилирует: проверяет
version/ELF/RPATH и упаковывает уже собранные outputs. Публикация и установка
артефактов являются отдельными действиями. `native-profile.json` хранит фактические
inputs сборки квалифицированных binaries, включая использованный Core SDK.
Текущий исходный SDK отдельно закрепляет свою зависимость в `sdk.json`;
повторная упаковка прежних native bytes не изменяет историю их сборки.

## Исходные SDK

`scripts/export-source-sdk.sh desktop-host OUTPUT` создаёт contracts/client/native
и точную копию чистых Core contracts; standalone manifests сохраняют edition2024,
Rust1.92, версии и внешние pins. Ни engine, ни NATS, ни Core binary binding в
этом SDK нет. `portable-supplier` содержит только production Cargo closure явной
Android native supplier сборки: Network library/runtime, contracts/native,
patched NATS и полный проверяемый Core binding/TLS/contracts SDK с исходным
inventory. Включены `build.rs`, только Android Core pins/descriptors, лицензии и
supplier lock. Linux CLI, client/FFI/helper, qualification, tests, workflows и
release/deployment инструменты в этот профиль не входят. Desktop профиль также
не содержит тестов или test targets поставщика. Внутренние `cfg(test)` модули
production source сохраняются без изменений; собственные тесты поставщик
запускает в своём checkout. Исходники engine Core не включаются.
SDK содержит `sdk.json`, краткое руководство, лицензии, supplier lock и
`source-inputs.sha256`; archive SHA фиксируется снаружи.
До owning compilation SDK является source candidate. Обычная APK сборка
использует готовые JNI outputs.

ABI metadata backing учитывает отдельные ограниченные event/cold мосты. Core
executor выделяется один раз на процесс; каждый Network RuntimeHandle
консервативно резервирует полный `ABI_EXECUTOR_STACK_BACKING`. Несколько handles
в одном процессе переучитывают эту фиксированную память, сохраняя локальный cap
без отдельного глобального ledger. Принятые default budgets не увеличиваются.

Android native supplier использует отдельные Core pins в
`dependencies/core/<Rust-target>.json` и точные supplier descriptors в
`dependencies/core/descriptors/`. Поддерживаются `aarch64-linux-android`
(`arm64-v8a`) и `x86_64-linux-android` (`x86_64`): minimum API31, NDK revision
30.0.16248370, ELF LOAD alignment16384. Каждый target получает собственные
artifact identity/library hash/descriptor hash; общий Core source SDK сохраняется
неизменённым. Перед Android build `SKVOZ_CORE_ARTIFACT` указывает на готовый
артефакт соответствующего target. Network сверяет полный descriptor hash и
Android profile, Core binding дополнительно проверяет library bytes.
Cross-compilation/ELF packaging не подтверждают Android loader/JNI/VPN lifecycle;
их проверяет Android supplier. Обычная APK сборка включает готовые JNI/Core
libraries, не запускает native compiler или Core source fallback.

Новые source SDK archives содержат только regular source files с mode0644;
executable scripts в профили не входят. Source inventory проверяет содержимое; это не native bundle
manifest и не квалификация executable trust. Уже выданные immutable SDK не
перепаковываются из-за изменения exporter metadata.

