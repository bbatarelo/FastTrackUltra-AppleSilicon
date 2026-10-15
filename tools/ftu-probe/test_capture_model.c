#include "capture_model.h"
#include <stdio.h>
#include <string.h>

static int failures;

#define CHECK(cond)                                                       \
    do {                                                                  \
        if (!(cond)) {                                                    \
            failures++;                                                   \
            fprintf(stderr, "%s:%d: CHECK failed: %s\n", __FILE__,        \
                    __LINE__, #cond);                                     \
        }                                                                 \
    } while (0)

static void check_invariants(const struct cap_model *m)
{
    size_t n[4] = { 0, 0, 0, 0 };
    size_t i;

    for (i = 0; i < m->count; i++)
        n[m->state[i]]++;
    CHECK(m->next <= m->count);
    CHECK(m->next == m->submitted + m->failed);
    CHECK(m->submitted == m->completed + m->pending);
    CHECK(n[CAP_NEW] == m->count - m->next);
    CHECK(n[CAP_PENDING] == m->pending);
    CHECK(n[CAP_COMPLETE] == m->completed);
    CHECK(n[CAP_SUBMIT_FAILED] == m->failed);
    CHECK(m->pending <= CAP_QUEUE_DEPTH);
    CHECK(m->failed <= 1);
    CHECK(m->failed == 0 || m->stopped);
}

static void submit_ok(struct cap_model *m, size_t index)
{
    CHECK(cap_model_can_submit(m, index));
    CHECK(cap_model_submit(m, index, true));
    CHECK(m->state[index] == CAP_PENDING);
    check_invariants(m);
}

static void complete_ok(struct cap_model *m, size_t index)
{
    CHECK(cap_model_complete(m, index));
    CHECK(m->state[index] == CAP_COMPLETE);
    check_invariants(m);
}

static void submit_rejected(struct cap_model *m, size_t index, bool success)
{
    struct cap_model before;

    memcpy(&before, m, sizeof before);
    CHECK(!cap_model_can_submit(m, index));
    CHECK(!cap_model_submit(m, index, success));
    CHECK(memcmp(&before, m, sizeof before) == 0);
}

static void complete_rejected(struct cap_model *m, size_t index)
{
    struct cap_model before;

    memcpy(&before, m, sizeof before);
    CHECK(!cap_model_complete(m, index));
    CHECK(memcmp(&before, m, sizeof before) == 0);
}

static void test_init_bounds(void)
{
    struct cap_model m, before;
    size_t i;

    memset(&m, 0x5A, sizeof m);
    memcpy(&before, &m, sizeof m);
    CHECK(!cap_model_init(&m, 0));
    CHECK(memcmp(&before, &m, sizeof m) == 0);
    CHECK(!cap_model_init(&m, CAP_MAX_BLOCKS + 1));
    CHECK(memcmp(&before, &m, sizeof m) == 0);
    CHECK(!cap_model_init(&m, (size_t)-1));
    CHECK(memcmp(&before, &m, sizeof m) == 0);
    CHECK(!cap_model_init(NULL, 1));

    CHECK(cap_model_init(&m, 1));
    CHECK(m.count == 1 && m.next == 0 && m.pending == 0);
    CHECK(m.submitted == 0 && m.completed == 0 && m.failed == 0);
    CHECK(!m.stopped);
    for (i = 0; i < CAP_MAX_BLOCKS; i++)
        CHECK(m.state[i] == CAP_NEW);
    CHECK(cap_model_can_release(&m));
    check_invariants(&m);

    CHECK(cap_model_init(&m, CAP_MAX_BLOCKS));
    CHECK(m.count == CAP_MAX_BLOCKS);
    check_invariants(&m);

    /* Re-init after use resets everything. */
    submit_ok(&m, 0);
    cap_model_stop(&m);
    CHECK(cap_model_init(&m, 2));
    CHECK(m.count == 2 && m.next == 0 && m.pending == 0 && !m.stopped);
    CHECK(m.state[0] == CAP_NEW);
    check_invariants(&m);
}

static void test_lifecycle_out_of_order(void)
{
    struct cap_model m;
    size_t i;

    CHECK(cap_model_init(&m, 8));
    for (i = 0; i < 4; i++)
        submit_ok(&m, i);
    CHECK(m.pending == CAP_QUEUE_DEPTH);
    submit_rejected(&m, 4, true);
    submit_rejected(&m, 4, false);
    CHECK(!cap_model_can_release(&m));

    complete_ok(&m, 2);
    complete_ok(&m, 0);
    CHECK(m.pending == 2 && m.completed == 2);
    submit_rejected(&m, 5, true); /* not sequential */
    submit_rejected(&m, 3, true); /* already used */
    submit_ok(&m, 4);
    submit_ok(&m, 5);
    complete_ok(&m, 5);
    complete_ok(&m, 3);
    complete_ok(&m, 1);
    complete_ok(&m, 4);
    submit_ok(&m, 6);
    submit_ok(&m, 7);
    CHECK(!cap_model_can_release(&m));
    complete_ok(&m, 7);
    complete_ok(&m, 6);
    CHECK(cap_model_can_release(&m));
    CHECK(m.submitted == 8 && m.completed == 8 && m.failed == 0);
    CHECK(!m.stopped);
}

static void test_failed_submit_after_three_pending(void)
{
    struct cap_model m;

    CHECK(cap_model_init(&m, 8));
    submit_ok(&m, 0);
    submit_ok(&m, 1);
    submit_ok(&m, 2);
    CHECK(cap_model_can_submit(&m, 3));
    CHECK(cap_model_submit(&m, 3, false));
    check_invariants(&m);
    CHECK(m.state[3] == CAP_SUBMIT_FAILED);
    CHECK(m.stopped && m.failed == 1 && m.next == 4);
    CHECK(m.pending == 3 && m.submitted == 3);
    submit_rejected(&m, 4, true);
    submit_rejected(&m, 4, false);
    complete_rejected(&m, 3); /* failed submit owes no callback */
    CHECK(!cap_model_can_release(&m));
    complete_ok(&m, 1);
    complete_ok(&m, 2);
    CHECK(!cap_model_can_release(&m));
    complete_ok(&m, 0);
    CHECK(cap_model_can_release(&m));
    CHECK(m.completed == 3 && m.failed == 1);
}

static void test_failed_first_submit(void)
{
    struct cap_model m;

    CHECK(cap_model_init(&m, 2));
    CHECK(cap_model_submit(&m, 0, false));
    check_invariants(&m);
    CHECK(m.stopped && m.pending == 0);
    CHECK(cap_model_can_release(&m));
    submit_rejected(&m, 1, true);
    complete_rejected(&m, 0);
}

static void test_stop_drain(void)
{
    struct cap_model m;

    CHECK(cap_model_init(&m, 8));
    submit_ok(&m, 0);
    submit_ok(&m, 1);
    cap_model_stop(&m);
    CHECK(m.stopped);
    check_invariants(&m);
    submit_rejected(&m, 2, true);
    submit_rejected(&m, 2, false);
    CHECK(!cap_model_can_release(&m));
    complete_ok(&m, 1);
    CHECK(!cap_model_can_release(&m));
    complete_ok(&m, 0);
    CHECK(cap_model_can_release(&m));
    cap_model_stop(&m);
    CHECK(m.stopped && m.completed == 2 && m.failed == 0);
    cap_model_stop(NULL);
}

static void test_duplicate_and_unsubmitted_completion(void)
{
    struct cap_model m;

    CHECK(cap_model_init(&m, 4));
    complete_rejected(&m, 0); /* NEW */
    submit_ok(&m, 0);
    complete_ok(&m, 0);
    complete_rejected(&m, 0); /* duplicate */
    complete_rejected(&m, 1); /* NEW */
    complete_rejected(&m, 4); /* past count */
    complete_rejected(&m, CAP_MAX_BLOCKS);
    complete_rejected(&m, (size_t)-1);
    CHECK(!cap_model_complete(NULL, 0));
    CHECK(m.completed == 1 && m.pending == 0);
}

static void test_exhaustion(void)
{
    struct cap_model m;
    size_t i;

    CHECK(cap_model_init(&m, 3));
    for (i = 0; i < 3; i++) {
        submit_ok(&m, i);
        complete_ok(&m, i);
    }
    submit_rejected(&m, 3, true);
    submit_rejected(&m, 3, false);
    submit_rejected(&m, 2, true);
    CHECK(cap_model_can_release(&m));
    CHECK(!m.stopped && m.next == 3);

    CHECK(cap_model_init(&m, 1));
    submit_ok(&m, 0);
    submit_rejected(&m, 1, true);
    complete_ok(&m, 0);
    submit_rejected(&m, 1, true);
}

static void test_bounds_and_null(void)
{
    struct cap_model m;

    CHECK(cap_model_init(&m, 4));
    CHECK(!cap_model_can_submit(&m, 4));
    CHECK(!cap_model_can_submit(&m, CAP_MAX_BLOCKS));
    CHECK(!cap_model_can_submit(&m, (size_t)-1));
    submit_rejected(&m, (size_t)-1, true);
    submit_rejected(&m, CAP_MAX_BLOCKS, false);
    CHECK(!cap_model_can_submit(NULL, 0));
    CHECK(!cap_model_submit(NULL, 0, true));
    CHECK(!cap_model_submit(NULL, 0, false));
    CHECK(!cap_model_can_release(NULL));
}

static void expect_packet(bool completed, bool ok, size_t actual,
                          enum cap_packet_kind kind, size_t frames,
                          bool accepted)
{
    struct cap_packet p = cap_packet_check(completed, ok, actual);

    CHECK(p.kind == kind);
    CHECK(p.audio_frames == frames);
    CHECK(p.accepted == accepted);
    if (p.kind != kind)
        fprintf(stderr, "  packet(%d,%d,%zu) -> %s\n", completed, ok,
                actual, cap_packet_name(p.kind));
}

static void test_packet_check(void)
{
    expect_packet(false, true, 24, CAP_PACKET_PENDING, 0, false);
    expect_packet(false, false, 400, CAP_PACKET_PENDING, 0, false);
    expect_packet(true, true, 313, CAP_PACKET_OVERSIZE, 0, false);
    expect_packet(true, false, 313, CAP_PACKET_OVERSIZE, 0, false);
    expect_packet(true, true, 336, CAP_PACKET_OVERSIZE, 0, false);
    expect_packet(true, true, (size_t)-1, CAP_PACKET_OVERSIZE, 0, false);
    expect_packet(true, false, 24, CAP_PACKET_ERROR, 0, false);
    expect_packet(true, false, 0, CAP_PACKET_ERROR, 0, false);
    expect_packet(true, false, 25, CAP_PACKET_ERROR, 0, false);
    expect_packet(true, true, 1, CAP_PACKET_UNALIGNED, 0, false);
    expect_packet(true, true, 25, CAP_PACKET_UNALIGNED, 0, false);
    expect_packet(true, true, 311, CAP_PACKET_UNALIGNED, 0, false);
    expect_packet(true, true, 0, CAP_PACKET_EMPTY, 0, true);
    expect_packet(true, true, 24, CAP_PACKET_OK, 1, true);
    expect_packet(true, true, 48, CAP_PACKET_OK, 2, true);
    expect_packet(true, true, 312, CAP_PACKET_OK, 13, true);
}

static void test_packet_names(void)
{
    static const enum cap_packet_kind kinds[] = {
        CAP_PACKET_OK, CAP_PACKET_EMPTY, CAP_PACKET_PENDING,
        CAP_PACKET_ERROR, CAP_PACKET_OVERSIZE, CAP_PACKET_UNALIGNED
    };
    size_t i, j, n = sizeof kinds / sizeof kinds[0];

    for (i = 0; i < n; i++) {
        CHECK(cap_packet_name(kinds[i]) != NULL);
        for (j = 0; j < i; j++)
            CHECK(strcmp(cap_packet_name(kinds[i]),
                         cap_packet_name(kinds[j])) != 0);
    }
    CHECK(cap_packet_name((enum cap_packet_kind)99) != NULL);
}

static void test_frame_conservation(void)
{
    static const size_t sizes[] = { 312, 0, 24, 300, 288, 313, 25, 48, 240 };
    const size_t nsizes = sizeof sizes / sizeof sizes[0];
    struct cap_model m;
    size_t queue[CAP_QUEUE_DEPTH];
    size_t qn = 0, round = 0, bytes = 0, frames = 0, accepted = 0;
    size_t actual, k;

    /* Per packet: accepted packets carry exactly actual/24 frames. */
    for (actual = 0; actual <= CAP_PACKET_BYTES + 30; actual++) {
        for (k = 0; k < 4; k++) {
            struct cap_packet p = cap_packet_check(k & 1u, (k & 2u) != 0,
                                                   actual);
            if (p.accepted) {
                CHECK(p.audio_frames * CAP_AUDIO_FRAME_BYTES == actual);
                CHECK(actual <= CAP_PACKET_BYTES);
            } else {
                CHECK(p.audio_frames == 0);
            }
        }
    }

    /* Full run of 256 blocks, out-of-order completion, mixed packets. */
    CHECK(cap_model_init(&m, CAP_MAX_BLOCKS));
    for (;;) {
        size_t pick, idx;
        struct cap_packet p;

        while (cap_model_can_submit(&m, m.next)) {
            queue[qn++] = m.next;
            CHECK(cap_model_submit(&m, m.next, true));
        }
        if (qn == 0)
            break;
        pick = round % 3 == 0 ? qn - 1 : 0;
        idx = queue[pick];
        memmove(&queue[pick], &queue[pick + 1],
                (qn - pick - 1) * sizeof queue[0]);
        qn--;
        CHECK(cap_model_complete(&m, idx));
        check_invariants(&m);
        p = cap_packet_check(true, idx % 7 != 3, sizes[idx % nsizes]);
        if (p.accepted) {
            accepted++;
            bytes += sizes[idx % nsizes];
            frames += p.audio_frames;
        }
        round++;
    }
    CHECK(round == CAP_MAX_BLOCKS);
    CHECK(m.completed == CAP_MAX_BLOCKS && m.submitted == CAP_MAX_BLOCKS);
    CHECK(cap_model_can_release(&m));
    CHECK(accepted > 0 && frames > 0);
    CHECK(frames * CAP_AUDIO_FRAME_BYTES == bytes);
}

int main(void)
{
    test_init_bounds();
    test_lifecycle_out_of_order();
    test_failed_submit_after_three_pending();
    test_failed_first_submit();
    test_stop_drain();
    test_duplicate_and_unsubmitted_completion();
    test_exhaustion();
    test_bounds_and_null();
    test_packet_check();
    test_packet_names();
    test_frame_conservation();
    if (failures != 0) {
        fprintf(stderr, "test_capture_model: %d failure(s)\n", failures);
        return 1;
    }
    printf("test_capture_model: all passed\n");
    return 0;
}
