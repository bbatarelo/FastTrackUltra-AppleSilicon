#include "../tools/hal-check/session_guard.h"
#include <assert.h>
#include <stdio.h>
int main(void) {
 struct session_guard g=session_guard_before_start("{\"phase\":0,\"epoch\":47}");
 assert(g.valid&&g.epoch==48);
 assert(session_guard_matches(g,"{\"phase\":2,\"epoch\":48,\"fault\":0}"));
 /* A fault and restart wholly between polls must not pass qualification. */
 assert(!session_guard_matches(g,"{\"phase\":2,\"epoch\":49,\"fault\":0}"));
 assert(session_guard_matches(g,"{\"phase\":0,\"epoch\":48,\"fault\":0}"));
 assert(!session_guard_matches(g,"{\"phase\":0,\"epoch\":49,\"fault\":0}"));
 g=session_guard_before_start("{\"phase\":2,\"epoch\":48}");assert(g.epoch==48);
 assert(!session_guard_matches(g,"{\"phase\":2,\"epoch\":47}"));
 /* Starting while already faulted/stopping is not a clean audio trial. */
 assert(!session_guard_before_start("{\"phase\":1,\"epoch\":48}").valid);
 assert(!session_guard_before_start("{\"phase\":3,\"epoch\":48}").valid);
 assert(!session_guard_before_start("{\"phase\":4,\"epoch\":48}").valid);
 assert(!session_guard_before_start("{\"phase\":5,\"epoch\":48}").valid);
 assert(!session_guard_before_start("{\"phase\":0}").valid);
 assert(!session_guard_before_start("{\"phase\":0,\"epoch\":18446744073709551615}").valid);
 assert(!session_guard_before_start("{\"phase\":0,\"epoch\":18446744073709551616}").valid);
 assert(!session_guard_before_start("{\"phase\":0,\"epoch\":-1}").valid);
 assert(!session_guard_before_start("{\"phase\":0,\"epoch\":1.5}").valid);
 assert(!session_guard_before_start("{\"phase\":0,\"epoch\":1e3}").valid);
 assert(!session_guard_matches(g,"{\"epoch\":48"));
 assert(session_guard_matches(g,"{ \"epoch\" : 48 }"));
 puts("Session continuity: epoch changes remain qualification failures after recovery: PASS");
}
