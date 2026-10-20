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
незакоммиченного кандидата; выпуск фиксирует owning commit. Объект `source`
содержит строго `repository`, `revision` и `tree_sha256`; дополнительные поля
отклоняются потребителем.

Consumer lock schema1 содержит ту же identity и `artifact:{url,sha256}`.
Относительный `file:` locator разрешается относительно каталога lock, независимо
от текущего рабочего каталога. Например, root `product-native-lock.json` использует
`file:artifacts/native/network-native.tar.gz`; lock `artifacts/network.lock.json`
для того же архива использует `file:native/network-native.tar.gz`.
Consumer сначала проверяет SHA256 всего архива, затем schema/tuple, digest
каждого файла, ожидаемые пути и mode. Разрешены только обычные файлы: без links,
absolute paths, `..`, duplicate paths и duplicate JSON fields. Manifest не более
64KiB, не более64 файлов и 256MiB суммарного распакованного содержимого. Missing,
hash/ABI/target mismatch останавливают packaging без source fallback.

## GitHub Releases и обновление Core

Workflow `Network` на `main` и при ручном запуске получает закреплённый Linux
архив Core и собирает runtime/helper в Rust1.92.0 / Debian Bookworm. Pull request
выполняет ту же проверку и сборку с `contents: read`; публикация доступна только
отдельному job доверенного `main`. Docker ограничен 2CPU, 3GiB RAM, без swap,
256 PID; каждый контейнер имеет deadline и проверку удаления. Профиль требует
GLIBC не новее2.34 для Core, runtime и helper.

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

`fetch` дополнительно сверяет SHA всего архива и descriptor с pin, а SDK — с
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
этом SDK нет. `portable-supplier` экспортирует полный Network source checkout
с patched NATS и Core binding/TLS/contracts для явной Android native supplier
сборки; исходники engine Core не включаются. SDK содержит `sdk.json`, текущую документацию, лицензии, supplier lock и
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

Новые source SDK archives нормализуют regular source files в0644, executable
scripts в0755. Source inventory проверяет содержимое; это не native bundle
manifest и не квалификация executable trust. Уже выданные immutable SDK не
перепаковываются из-за изменения exporter metadata.

