// OpenCleaner engine C ABI. Requests, results and events are UTF-8 JSON.
#ifndef OC_ENGINE_H
#define OC_ENGINE_H

#include <stdint.h>

typedef struct OcEngine OcEngine;

// Gets every event of a job, on the job's thread; the last one is {"event":"done"}.
// event_json is only valid during the call.
typedef void (*OcEventFn)(void *ctx, const char *event_json);

OcEngine *oc_engine_new(const char *config_json);
void oc_engine_free(OcEngine *engine);

// Quick request, answered at once. Free the result with oc_free.
char *oc_call(OcEngine *engine, const char *request_json);

// Long request as a job on its own thread. Returns the job id for oc_cancel (0 when
// engine is NULL). engine and ctx must stay valid until the "done" event; the engine is
// not used after it, so it may be freed then.
uint64_t oc_start(OcEngine *engine, const char *request_json, OcEventFn callback, void *ctx);
void oc_cancel(OcEngine *engine, uint64_t job);

void oc_free(char *s);

#endif
