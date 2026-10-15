#ifndef FTU_FEEDBACK_PROFILE_H
#define FTU_FEEDBACK_PROFILE_H
#include "capture_model.h"
bool feedback_profile_validate(const uint8_t *p, size_t n);
struct cap_packet feedback_packet_check(bool completed, bool status_ok, size_t actual);
#endif
