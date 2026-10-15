#ifndef FTU_CAPTURE_MODEL_H
#define FTU_CAPTURE_MODEL_H
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#define CAP_MAX_BLOCKS 256u
#define CAP_QUEUE_DEPTH 4u
#define CAP_PACKET_BYTES 312u
#define CAP_AUDIO_FRAME_BYTES 24u
enum cap_block_state { CAP_NEW, CAP_PENDING, CAP_COMPLETE, CAP_SUBMIT_FAILED };
struct cap_model {
    enum cap_block_state state[CAP_MAX_BLOCKS];
    size_t count, next, pending, submitted, completed, failed;
    bool stopped;
};
bool cap_model_init(struct cap_model *m, size_t count);
bool cap_model_can_submit(const struct cap_model *m, size_t index);
/* True iff the attempt was recorded; a recorded failure (success == false)
 * also returns true. False means the call was invalid and m is unchanged. */
bool cap_model_submit(struct cap_model *m, size_t index, bool success);
bool cap_model_complete(struct cap_model *m, size_t index);
void cap_model_stop(struct cap_model *m);
bool cap_model_can_release(const struct cap_model *m);
enum cap_packet_kind { CAP_PACKET_OK, CAP_PACKET_EMPTY, CAP_PACKET_PENDING,
                       CAP_PACKET_ERROR, CAP_PACKET_OVERSIZE, CAP_PACKET_UNALIGNED };
struct cap_packet { enum cap_packet_kind kind; size_t audio_frames; bool accepted; };
struct cap_packet cap_packet_check(bool completed, bool status_ok, size_t actual);
const char *cap_packet_name(enum cap_packet_kind kind);
#endif
