#ifndef FTU_CAPTURE_PROFILE_H
#define FTU_CAPTURE_PROFILE_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
/* Validate only the fixed first-experiment capture profile, losslessly retain
 * the original descriptor elsewhere. No claim to general UAC validation. */
bool cap_profile_validate(const uint8_t *bytes, size_t length);
#endif
