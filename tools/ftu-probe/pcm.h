#ifndef FTU_PCM_H
#define FTU_PCM_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
/* Scalar pointers must contain exactly three accessible bytes. */
int32_t pcm_s24_decode(const uint8_t p[3]);
float pcm_s24_float(const uint8_t p[3]);
void pcm_s24_encode(float sample, uint8_t p[3]);
/* Packed input/output is interleaved; float capacities/strides count float
 * elements, byte capacities count bytes. Validate every extent before writes.
 * Float address = frame * frame_stride + channel * channel_stride.
 * Reject overlapping float sample addresses, overflow, insufficient bounds,
 * zero channels, and non-whole byte capacities (partial packed frames).
 * Zero frames is valid with nonzero channels and zero capacities/null pointers.
 * A byte capacity is whole when it is a multiple of channels*3; it may exceed
 * frames*channels*3. Packed samples are little-endian. The byte buffer and the
 * float buffer must not overlap; this is caller-disallowed and not detected.
 */
bool pcm_decode_frames(const uint8_t *src, size_t src_bytes, float *dst,
                       size_t dst_count, size_t frames, size_t channels,
                       size_t frame_stride, size_t channel_stride);
bool pcm_encode_frames(const float *src, size_t src_count, uint8_t *dst,
                       size_t dst_bytes, size_t frames, size_t channels,
                       size_t frame_stride, size_t channel_stride);
#endif
