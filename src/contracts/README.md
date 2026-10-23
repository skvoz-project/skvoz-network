# Схемы Network

`skvoz-network-contracts` — единственный источник строгих StartupConfig3,
API1, helper1, enrollment4 и network6 DTO и валидаторов. Runtime и helper
используют те же типы: parser не копируется в привилегированный процесс.

Зависимости ограничены Serde/JSON, getrandom и чистым Core contracts SDK.
Engine, native Core library, async-nats и Tokio здесь отсутствуют. Core владеет
DTO профиля и формулой backing; supplier проверяет её против собственной layout.
Проверки полномочий connected owner, journal и kernel остаются в helper.

Канонический контракт: [сетевой runtime](../../docs/network-runtime.md).
