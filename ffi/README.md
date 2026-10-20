# Native ABI1

Экспериментальный Linux ABI для того же Rust actor из `skvoz-network`.
Заголовок: [include/skvoz_network.h](include/skvoz_network.h). Компонент собирает
`rlib`, `cdylib` и `staticlib`; протокол/API, лимиты и транспорт остаются общими
с Rust embedding. Android/JNI и другие платформы здесь не квалифицированы.
Общий контракт: [сетевой runtime](../docs/network-runtime.md).

```sh
cargo build -p skvoz-network-ffi --locked
cargo test -p skvoz-network-ffi --locked
```

Rust tests вызывают настоящие C ABI exports: HELLO до Ready, retained buffer
queries, owned FD и отвержение stale handle. Отдельный real-NATS case проверяет
binary echo и half-close; [условия запуска](../docs/verification.md).

`skvoz_network_create(config,len,helper_fd,out_handle)` копирует startup JSON
до 32768 bytes. Для client и TCP-only server `helper_fd=-1`; server с IP backend
передаёт borrowed connected helper FD. Библиотека создаёт собственный CLOEXEC
duplicate, проверяет роль и запускает один actor. Оригинал остаётся у caller,
который закрывает его после успешного создания. Handle — индекс с поколением,
не адрес объекта; после `destroy` он не используется повторно.

`request` копирует API1 JSON и duplicate необязательного borrowed FD до возврата.
Успешный admission возвращает ID запроса; его response приходит через
`next_event` вместе с остальными событиями. Пакеты через эту API не передаются.

`next_event` принимает caller-owned buffer и ждёт не более 1000 ms; timeout0
проверяет очередь без ожидания. Нулевой buffer с capacity0 позволяет получить
необходимый размер. `INSUFFICIENT_BUFFER` оставляет сообщение и его FD в той же
ограниченной очереди actor. Только успешный вызов передаёт owned `out_fd` один
раз; caller закрывает его. `out_fd=-1` означает отсутствие FD. JSON не содержит
завершающего NUL.

Статусы: 0 — success, 1 — insufficient buffer, 2 — timeout, 3 — invalid argument,
4 — closed/stale handle, 5 — overloaded/internal failure. Ошибка выполнения
корректного API запроса передаётся в JSON response. Pointer storage, alignment,
размер и отсутствие пересечения областей — обязанности C caller; библиотека
проверяет NULL и числовые границы, не хранит caller pointers и перехватывает
Rust unwind на границе ABI. Input находится в одной allocation и не изменяется
параллельно во время вызова; output принадлежит вызывающему потоку на время
записи. Проверка числового адреса не проверяет доступность памяти.

Владелец сериализует `request`/`next_event` одного handle и прекращает polling
перед `destroy`. Destroy закрывает ещё не переданные FD; успех подтверждает
завершение того же actor. Внутренний mutex также сериализует одновременно вызванные операции одного
handle: `request`/`destroy` могут ждать текущий poll до 1000 ms, затем ограниченное
завершение actor. Другие handles не держат этот mutex. Неподтверждённая очистка
возвращает статус 5; уже закрытый actor может вернуть статус 4. В обоих случаях
handle уже retired и повторный `destroy` возвращает статус 4.

[Публичные векторы](tests/vectors/requests.json) проверяют строгую API1 schema;
проверки реального TLS/NATS транспорта запускаются явно по
[руководству проверки](../docs/verification.md).
