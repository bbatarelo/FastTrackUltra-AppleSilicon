#include "pcm.h"

/* Packed samples are little-endian signed 24-bit. The packed byte buffer and
 * the float buffer must not overlap; this is caller-disallowed, not detected. */

#define PCM_SCALE 8388608.0
#define PCM_MAX 8388607
#define PCM_MIN (-8388607 - 1)

int32_t pcm_s24_decode(const uint8_t p[3])
{
    uint32_t u = (uint32_t)p[0] | ((uint32_t)p[1] << 8) | ((uint32_t)p[2] << 16);

    return (int32_t)(u & 0x7FFFFFu) - (int32_t)(u & 0x800000u);
}

float pcm_s24_float(const uint8_t p[3])
{
    return (float)pcm_s24_decode(p) / 8388608.0f;
}

static int32_t pcm_quantize(float sample)
{
    double d, a;
    int32_t t;

    if (sample != sample)
        return 0;
    d = (double)sample * PCM_SCALE;
    if (d >= (double)PCM_MAX)
        return PCM_MAX;
    if (d <= (double)PCM_MIN)
        return PCM_MIN;
    /* |d| < 2^23 here, so truncation and the fraction are exact. */
    a = d < 0.0 ? -d : d;
    t = (int32_t)a;
    if (a - (double)t >= 0.5)
        t++;
    return d < 0.0 ? -t : t;
}

void pcm_s24_encode(float sample, uint8_t p[3])
{
    uint32_t u = (uint32_t)pcm_quantize(sample);

    p[0] = (uint8_t)(u & 0xFFu);
    p[1] = (uint8_t)((u >> 8) & 0xFFu);
    p[2] = (uint8_t)((u >> 16) & 0xFFu);
}

static size_t pcm_gcd(size_t a, size_t b)
{
    while (b != 0) {
        size_t t = a % b;
        a = b;
        b = t;
    }
    return a;
}

/* Validates both buffers for a transfer; performs no writes. */
static bool pcm_check(const void *bytes, size_t byte_cap, const void *floats,
                      size_t float_cap, size_t frames, size_t channels,
                      size_t fs, size_t cs)
{
    size_t frame_bytes, max_f, max_c;

    if (channels == 0 || channels > SIZE_MAX / 3u)
        return false;
    if ((bytes == NULL && byte_cap != 0) || (floats == NULL && float_cap != 0))
        return false;
    frame_bytes = channels * 3u;
    if (byte_cap % frame_bytes != 0)
        return false;
    if (frames == 0)
        return true;
    if (byte_cap / frame_bytes < frames)
        return false;

    /* Addresses f*fs + c*cs collide iff some 0<df<frames, 0<dc<channels
     * satisfy df*fs == dc*cs; the smallest such pair is (cs/g, fs/g). */
    if (frames > 1 && fs == 0)
        return false;
    if (channels > 1 && cs == 0)
        return false;
    if (frames > 1 && channels > 1) {
        size_t g = pcm_gcd(fs, cs);
        if (cs / g < frames && fs / g < channels)
            return false;
    }

    max_f = frames - 1;
    max_c = channels - 1;
    if (max_f != 0 && fs > SIZE_MAX / max_f)
        return false;
    if (max_c != 0 && cs > SIZE_MAX / max_c)
        return false;
    if (max_f * fs > SIZE_MAX - max_c * cs)
        return false;
    size_t last = max_f * fs + max_c * cs;
    /* Element offsets must also fit when scaled to byte addresses. */
    return last < float_cap && last < SIZE_MAX / sizeof(float);
}

bool pcm_decode_frames(const uint8_t *src, size_t src_bytes, float *dst,
                       size_t dst_count, size_t frames, size_t channels,
                       size_t frame_stride, size_t channel_stride)
{
    size_t f, c;

    if (!pcm_check(src, src_bytes, dst, dst_count, frames, channels,
                   frame_stride, channel_stride))
        return false;
    for (f = 0; f < frames; f++)
        for (c = 0; c < channels; c++)
            dst[f * frame_stride + c * channel_stride] =
                pcm_s24_float(src + (f * channels + c) * 3u);
    return true;
}

bool pcm_encode_frames(const float *src, size_t src_count, uint8_t *dst,
                       size_t dst_bytes, size_t frames, size_t channels,
                       size_t frame_stride, size_t channel_stride)
{
    size_t f, c;

    if (!pcm_check(dst, dst_bytes, src, src_count, frames, channels,
                   frame_stride, channel_stride))
        return false;
    for (f = 0; f < frames; f++)
        for (c = 0; c < channels; c++)
            pcm_s24_encode(src[f * frame_stride + c * channel_stride],
                           dst + (f * channels + c) * 3u);
    return true;
}
