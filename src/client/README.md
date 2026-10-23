# Host SDK enrollment

`skvoz-network-client` предоставляет Linux host `RuntimeClient::enroll` без
Core engine, NATS и TLS source build. Установщик закрепляет и проверяет готовый
Network archive, сохраняет `/opt/skvoz-native/bin` рядом с `lib`, затем передаёт
SDK абсолютный доверенный путь к `skvoz-network-runtime`. SDK принимает только
обычный executable без group/other write; доверие к каталогам и digest обеспечивает
установщик по product lock.

Операция использует внутреннюю `--enroll-stdio` границу того же executable.
Credentials передаются только в ограниченном stdin JSON, stderr не сохраняется.
Результат — строгий JSON с обязательными nullable `result`/`error`; payload
ограничен 2048 байт, вход — 8192. Проверяются enrollment4 и точный набор
Network0.7.0/API1/network6/Core5.0.0. Ошибки сохраняют прежние идентификаторы.

SDK допускает одну операцию на процесс. Один owned worker обслуживает
неблокирующие pipes; общий срок — 20 секунд. Отмена, ошибка и превышение срока
закрывают pipes, убивают и reap дочерний процесс перед освобождением admission.
Прямой async enrollment сохранён в supplier для native Android и runtime:
проверенный TLS, INFO→TLS→CONNECT, два ограниченных auth retry с паузами1/2с.

```rust,ignore
let sdk = skvoz_network_client::RuntimeClient::new(
    "/opt/skvoz-native/bin/skvoz-network-runtime",
)?;
let enrollment = sdk.enroll(&credentials, &device_token).await?;
```

Rust process checks из `src/`: `cargo test -p skvoz-network-client --locked`.
Они проверяют процессную границу с Rust fixture и не заменяют TLS/NATS integration.
