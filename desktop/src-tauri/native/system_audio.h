#pragma once
#include <stdint.h>
// Local system-output capture only. All calls except the IO callback are serialized.
int32_t eden_audio_start(void **handle, double *sample_rate);
uint32_t eden_audio_read(void *handle, float *stereo, uint32_t max_frames);
void eden_audio_set_paused(void *handle, int32_t paused);
void eden_audio_stop(void *handle);
