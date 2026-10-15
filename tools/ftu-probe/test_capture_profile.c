#include "capture_profile.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv)
{
    assert(argc == 2);
    FILE *f = fopen(argv[1], "rb");
    assert(f);
    uint8_t original[341], changed[341];
    size_t n = fread(original, 1, sizeof original, f);
    assert(!ferror(f) && fclose(f) == 0 && n == 340);
    assert(cap_profile_validate(original, n));
    assert(!cap_profile_validate(NULL, 0));
    for (size_t i = 0; i < n; i++) assert(!cap_profile_validate(original, i));
    /* Independently indexed fields in the real desktop capture: idle endpoint
     * count, active identity/count/class/protocol, PCM tag/type/channels/width,
     * 48k rate, endpoint address/attributes/capacity/interval. */
    const size_t fields[] = {149,156,157,158,159,160,161,168,169,
                            173,174,175,176,177,181,192,193,194,195,196};
    for (size_t i = 0; i < sizeof fields / sizeof fields[0]; i++) {
        memcpy(changed, original, n);
        changed[fields[i]] ^= 1;
        assert(!cap_profile_validate(changed, n));
    }
    memcpy(changed, original, n);
    changed[207] = 1; /* Turn alternate 2 into duplicate alternate 1. */
    assert(!cap_profile_validate(changed, n));
    /* Bounds exercise with the advertised total adjusted, so the walker sees
     * truncated interiors instead of rejecting only at the outer length. */
    for (size_t i = 9; i < n; i++) {
        memcpy(changed, original, n);
        changed[2] = (uint8_t)i; changed[3] = (uint8_t)(i >> 8);
        (void)cap_profile_validate(changed, i);
    }
    uint32_t rng = 2080;
    for (size_t trial = 0; trial < 25000; trial++) {
        memcpy(changed, original, n);
        for (unsigned j = 0; j < 4; j++) {
            rng = rng * 1664525u + 1013904223u;
            size_t off = rng % n;
            rng = rng * 1664525u + 1013904223u;
            changed[off] = (uint8_t)(rng >> 24);
        }
        (void)cap_profile_validate(changed, n);
    }
    puts("test_capture_profile: fixture, rejection and 25000 bounds mutations passed");
    return 0;
}
