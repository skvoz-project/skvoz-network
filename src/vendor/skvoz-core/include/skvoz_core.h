#ifndef SKVOZ_CORE_H
#define SKVOZ_CORE_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
#define SKVOZ_CORE_ABI_VERSION 1u
#define SKVOZ_CORE_MAX_DTO_BYTES 65536u
#define SKVOZ_CORE_MAX_EVENT_BYTES 16384u
#define SKVOZ_CORE_MAX_EVENTS 256u
#define SKVOZ_CORE_OK 0
#define SKVOZ_CORE_PENDING 1
#define SKVOZ_CORE_WOULD_BLOCK 2
#define SKVOZ_CORE_ERROR (-1)
#define SKVOZ_CORE_INVALID (-2)
#define SKVOZ_CORE_BUSY (-3)
#define SKVOZ_CORE_PANIC (-4)
typedef struct { uint64_t epoch_high, epoch_low, incarnation, peer, stream; } SkvozCoreKey;
typedef struct { uint8_t *data; size_t len; } SkvozCoreBuffer;
typedef struct { uint64_t value; SkvozCoreKey key; SkvozCoreBuffer buffer; } SkvozCoreReply;
typedef struct {
    void *context;
    void (*retain)(void *);
    void (*wake)(void *);
    void (*release)(void *);
} SkvozCoreWake;
typedef struct {
    SkvozCoreKey key;
    uint32_t kind, reason;
    uint64_t offset;
    const uint8_t *data;
    size_t len;
} SkvozCoreEvent;
/* Each handle has one exclusive owner. Callbacks must not reenter Core or unwind.
 * Wake callbacks are thread-safe and balanced by retain/release. Event payloads
 * are borrowed only for receive(), never acknowledged by event extraction.
 * Reply must be zero-initialized. Free its buffer exactly once with buffer_free.
 * The library must stay loaded until all handles and callback references die. */
uint32_t skvoz_core_v1_version(void);
uint64_t skvoz_core_v1_capabilities(void);
void skvoz_core_v1_buffer_free(SkvozCoreBuffer buffer);
int32_t skvoz_core_v1_runtime_new(void **out);
int32_t skvoz_core_v1_runtime_free(void *runtime);
/* kind: 0 connect (bounded JSON config+limits), 1 turn, 2 shutdown. */
int32_t skvoz_core_v1_start(void *runtime, uint32_t kind, uint64_t wait_ms,
    const uint8_t *data, size_t len, void **operation, SkvozCoreReply *reply);
/* host_poll runs only at Core's original idle wait after guarded output. */
int32_t skvoz_core_v1_poll(void *operation, SkvozCoreWake wake,
    void *host_context, bool (*host_poll)(void *), SkvozCoreReply *reply);
/* Frees/cancels the original future and releases the owner's exclusive borrow. */
int32_t skvoz_core_v1_operation_free(void *operation);
/* command values are defined below. send accepts at most 16384 raw bytes. */
int32_t skvoz_core_v1_call(void *runtime, uint32_t command, SkvozCoreKey key,
    uint64_t argument, const uint8_t *data, size_t len, SkvozCoreReply *reply);
int32_t skvoz_core_v1_events(void *runtime, size_t maximum, void *context,
    bool (*admit)(void *, SkvozCoreKey, size_t), void (*receive)(void *, const SkvozCoreEvent *));
enum SkvozCoreCommand {
    SKVOZ_CORE_JOIN=1, SKVOZ_CORE_REVOKE=2, SKVOZ_CORE_AUTHORIZE=3,
    SKVOZ_CORE_TERMINATE=4, SKVOZ_CORE_EPOCH=5, SKVOZ_CORE_STATUS=6,
    SKVOZ_CORE_RESOURCES=7, SKVOZ_CORE_PEER_STATUS=8, SKVOZ_CORE_PEER_READY=9,
    SKVOZ_CORE_PEER_DRAINED=10, SKVOZ_CORE_OPEN=11, SKVOZ_CORE_ACCEPT=12,
    SKVOZ_CORE_REJECT=13, SKVOZ_CORE_SEND=14, SKVOZ_CORE_FINISH=15,
    SKVOZ_CORE_CONSUME=16, SKVOZ_CORE_CLOSE=17, SKVOZ_CORE_SNAPSHOT=18,
    SKVOZ_CORE_STREAM_DIAGNOSTICS=19, SKVOZ_CORE_LIMITS=20,
    SKVOZ_CORE_PEER_LIMITS=21, SKVOZ_CORE_TRANSPORT_LOST=22,
    SKVOZ_CORE_DIAGNOSTICS=23, SKVOZ_CORE_SET_DIAGNOSTICS=24,
    SKVOZ_CORE_DELIVERY_STATUS=25
};
#ifdef __cplusplus
}
#endif
#endif
