#include "capture_model.h"

bool cap_model_init(struct cap_model *m, size_t count)
{
    size_t i;

    if (m == NULL || count == 0 || count > CAP_MAX_BLOCKS)
        return false;
    for (i = 0; i < CAP_MAX_BLOCKS; i++)
        m->state[i] = CAP_NEW;
    m->count = count;
    m->next = 0;
    m->pending = 0;
    m->submitted = 0;
    m->completed = 0;
    m->failed = 0;
    m->stopped = false;
    return true;
}

bool cap_model_can_submit(const struct cap_model *m, size_t index)
{
    return m != NULL && !m->stopped && index == m->next &&
           m->next < m->count && m->pending < CAP_QUEUE_DEPTH;
}

/* Returns true when the submission attempt was recorded. A failed attempt
 * (success == false) is recorded too: it consumes the index and stops. */
bool cap_model_submit(struct cap_model *m, size_t index, bool success)
{
    if (!cap_model_can_submit(m, index))
        return false;
    m->next++;
    if (success) {
        m->state[index] = CAP_PENDING;
        m->pending++;
        m->submitted++;
    } else {
        m->state[index] = CAP_SUBMIT_FAILED;
        m->failed++;
        m->stopped = true;
    }
    return true;
}

bool cap_model_complete(struct cap_model *m, size_t index)
{
    if (m == NULL || index >= m->count || m->state[index] != CAP_PENDING)
        return false;
    m->state[index] = CAP_COMPLETE;
    m->pending--;
    m->completed++;
    return true;
}

void cap_model_stop(struct cap_model *m)
{
    if (m != NULL)
        m->stopped = true;
}

bool cap_model_can_release(const struct cap_model *m)
{
    return m != NULL && m->pending == 0;
}

struct cap_packet cap_packet_check(bool completed, bool status_ok, size_t actual)
{
    struct cap_packet p = { CAP_PACKET_OK, 0, false };

    if (!completed)
        p.kind = CAP_PACKET_PENDING;
    else if (actual > CAP_PACKET_BYTES)
        p.kind = CAP_PACKET_OVERSIZE;
    else if (!status_ok)
        p.kind = CAP_PACKET_ERROR;
    else if (actual % CAP_AUDIO_FRAME_BYTES != 0)
        p.kind = CAP_PACKET_UNALIGNED;
    else if (actual == 0) {
        p.kind = CAP_PACKET_EMPTY;
        p.accepted = true;
    } else {
        p.audio_frames = actual / CAP_AUDIO_FRAME_BYTES;
        p.accepted = true;
    }
    return p;
}

const char *cap_packet_name(enum cap_packet_kind kind)
{
    switch (kind) {
    case CAP_PACKET_OK: return "OK";
    case CAP_PACKET_EMPTY: return "EMPTY";
    case CAP_PACKET_PENDING: return "PENDING";
    case CAP_PACKET_ERROR: return "ERROR";
    case CAP_PACKET_OVERSIZE: return "OVERSIZE";
    case CAP_PACKET_UNALIGNED: return "UNALIGNED";
    }
    return "UNKNOWN";
}
