#include "feedback_profile.h"

bool feedback_profile_validate(const uint8_t *p, size_t n)
{
    if (!p || n < 9 || p[0] != 9 || p[1] != 2 ||
        ((size_t)p[2] | ((size_t)p[3] << 8)) != n) return false;
    unsigned idle = 0, active = 0, general = 0, format = 0, out = 0, in = 0;
    bool target = false;
    for (size_t off = 9; off < n;) {
        if (n - off < 2 || p[off] < 2 || p[off] > n - off) return false;
        const uint8_t *d = p + off;
        size_t len = d[0];
        if (d[1] == 4) {
            if (len < 9) return false;
            target = d[2] == 1 && d[3] == 1;
            if (d[2] == 1 && d[3] == 0) {
                if (++idle != 1 || d[4] != 0) return false;
            }
            if (target && (++active != 1 || d[4] != 2 || d[5] != 0xff ||
                           d[6] != 2 || d[7] != 0)) return false;
        } else if (target && d[1] == 0x24) {
            if (len < 3) return false;
            if (d[2] == 1) {
                if (++general != 1 || len < 7 || d[5] != 1 || d[6] != 0)
                    return false; /* AS_GENERAL PCM format tag */
            } else if (d[2] == 2) {
                if (++format != 1 || len < 8 || d[3] != 1 || d[4] != 8 ||
                    d[5] != 3 || d[6] != 24 || d[7] == 0 ||
                    len != 8u + 3u * d[7]) return false;
                bool rate = false;
                for (size_t i = 8; i < len; i += 3)
                    if (((uint32_t)d[i] | ((uint32_t)d[i+1] << 8) |
                         ((uint32_t)d[i+2] << 16)) == 48000) rate = true;
                if (!rate) return false;
            }
        } else if (target && d[1] == 5) {
            if (len < 7 || d[6] != 1) return false;
            if (d[2] == 0x01) {
                if (++out != 1 || d[3] != 5 || d[4] != 0x38 || d[5] != 1) return false;
            } else if (d[2] == 0x82) {
                if (++in != 1 || d[3] != 1 || d[4] != 3 || d[5] != 0) return false;
            } else return false;
        }
        off += len;
    }
    return idle == 1 && active == 1 && general == 1 && format == 1 && out == 1 && in == 1;
}

/* Accepted means bounded observed bytes, not a validated feedback encoding.
 * Short payloads remain evidence and are never padded into numeric values. */
struct cap_packet feedback_packet_check(bool completed, bool status_ok, size_t actual)
{
    struct cap_packet p = { CAP_PACKET_OK, 0, false };
    if (!completed) p.kind = CAP_PACKET_PENDING;
    else if (actual > 3) p.kind = CAP_PACKET_OVERSIZE;
    else if (!status_ok) p.kind = CAP_PACKET_ERROR;
    else {
        p.kind = actual ? CAP_PACKET_OK : CAP_PACKET_EMPTY;
        p.accepted = true;
    }
    return p;
}
